use heapless::{Vec, spsc::{Producer, Consumer}};
use heapless::consts::*;

const HEADER_BYTE: u8 = 0x0f;
const FOOTER_BYTE: u8 = 0x00;

const CHANNEL_AMOUNT: u8 = 16;
const CHANNEL_BYTE_COUNT: usize = 22;

#[derive(Debug, PartialEq)]
pub enum Error {
    MissingStopByte,
    Failsafe,
    VecFull(u8),
    ResultTxFull,
}
pub type Result<T> = core::result::Result<T, Error>;

#[derive(PartialEq, Debug, Clone)]
pub struct SbusMessage {
    pub channels: [u16; 16],
    pub digital_channels: [bool; 2],
    pub failsafe: bool
}

impl Default for SbusMessage {
    fn default() -> Self {
        Self {
            channels: [0; 16],
            digital_channels: [false; 2],
            failsafe: true
        }
    }
}

pub struct SbusDecoder<'a, E> {
    byte_rx: Consumer<'a, core::result::Result<u8, E>, U25>,
    result_tx: Producer<'a, Result<SbusMessage>, U1>,
    state: DecoderState,
    current_message: SbusMessage
}

impl<'a, E> SbusDecoder<'a, E> {
    pub fn new(
        byte_rx: Consumer<'a, core::result::Result<u8, E>, U25>,
        result_tx: Producer<'a, Result<SbusMessage>, U1>
    ) -> Self {
        Self {
            byte_rx,
            result_tx,
            state: DecoderState::WaitForHeader,
            current_message: Default::default()
        }
    }

    /**
      Process the bytes that have been sent over the byte channel. If a full frame
      has been received, or some bytes were invalid, the frame or error are sent
      over the message channel.

      If the message can't be sent over the message channel, an error is returned.
    */
    pub fn process(&mut self) -> Result<()> {
        loop {
            // Dequeue the next byte. If no byte is in the queue, exit.
            //
            // If the next byte is valid, continue parsing
            //
            // If an error was received from the byte sender, go into recovery
            // and continue parsing the next byte
            let byte = match self.byte_rx.dequeue() {
                Some(byte) => match byte {
                    Ok(byte) => byte,
                    Err(_) => {
                        self.state = DecoderState::Recover;
                        continue
                    }
                },
                None => break Ok(())
            };

            let new_state = match self.state.clone() {
                DecoderState::WaitForHeader => {
                    if byte == HEADER_BYTE {
                        DecoderState::Channel(Vec::default())
                    }
                    else {
                        // We expected a header byte but it did not arrive, go into
                        // recovery mode
                        DecoderState::Recover
                    }
                }
                DecoderState::Channel(mut previous_bytes) => {
                    if previous_bytes.len() < CHANNEL_BYTE_COUNT {
                        // We are still expecting more bytes with channel values,
                        // try to decode and store them.
                        if let Err(byte) = previous_bytes.push(byte) {
                            break Err(Error::VecFull(byte))
                        }
                        DecoderState::Channel(previous_bytes)
                    }
                    else {
                        // This was the last channel byte, decode channels
                        // and add the digital channels
                        decode_channels(&mut self.current_message, previous_bytes);
                        decode_digital_byte(&mut self.current_message, byte);
                        DecoderState::WaitForFooter
                    }
                }
                DecoderState::WaitForFooter => {
                    if byte == FOOTER_BYTE {
                        // We received a footer byte, decode the data
                        if !self.current_message.failsafe {
                            // We did not failsafe, send the resulting message.
                            let enqueue_result = self.result_tx.enqueue(
                                Ok(self.current_message.clone())
                            );
                            if let Err(_) = enqueue_result {
                                break Err(Error::ResultTxFull)
                            }
                        }
                        else {
                            // We failsafed, try to send that message
                            let enqueue_result = self.result_tx.enqueue(Err(Error::Failsafe));
                            if let Err(_) = enqueue_result {
                                break Err(Error::ResultTxFull)
                            }
                        }

                        // Wait for the next frame
                        DecoderState::WaitForHeader
                    }
                    else {
                        // We did not get a stop byte, try to relay that error
                        let enqueue_result = self.result_tx.enqueue(Err(Error::MissingStopByte));
                        if let Err(_) = enqueue_result {
                            break Err(Error::ResultTxFull)
                        }
                        DecoderState::Recover
                    }
                }
                DecoderState::Recover => {
                    // We need to see a sequence of FOOTER->HEADER to know that
                    // we are in a valid state
                    if byte == FOOTER_BYTE {
                        DecoderState::WaitForHeader
                    }
                    else {
                        DecoderState::Recover
                    }
                }
            };

            // Update the state
            self.state = new_state;
        }
    }
}

#[derive(Clone)]
enum DecoderState {
    WaitForHeader,
    /// Waiting for the specified channel. The second element is the amount of
    /// bytes already consumed. The last element is the partial
    /// channel that has already been decoded
    Channel(Vec<u8, U22>),
    /// Waiting for the last byte containing digital channels and failsafe
    WaitForFooter,
    Recover
}


fn decode_channels(message: &mut SbusMessage, bytes: Vec<u8, U22>) {
    for channel in 0..CHANNEL_AMOUNT {
        let offset = channel * 11;
        let first_byte_offset = offset % 8;
        let first_byte_index = offset / 8;

        let bits_from_next_bytes = 11 - (8 - first_byte_offset);

        let first_byte_mask = 0xff << first_byte_offset;
        let second_byte_mask = 0xff >> (8 - bits_from_next_bytes.min(8));

        let from_first_byte =
            ( (bytes[first_byte_index as usize] & first_byte_mask)
              >> first_byte_offset
            ) as u16;
        let from_second_byte =
            ( (bytes[1 + first_byte_index as usize] & second_byte_mask) as u16
            ) << (8 - first_byte_offset);

        let from_third_byte = if bits_from_next_bytes > 8 {
            let bits_from_third_byte = bits_from_next_bytes - 8;
            let third_byte_mask = 0xff >> (8  - bits_from_third_byte);
            ( (bytes[2 + first_byte_index as usize] & third_byte_mask) as u16
            ) << (11 - bits_from_third_byte)
        }
        else {
            0
        };

        message.channels[channel as usize]
            = from_first_byte
            | from_second_byte
            | from_third_byte
    }
}
fn decode_digital_byte(message: &mut SbusMessage, byte: u8) {
    message.digital_channels[0] = (byte & 0b001) != 0;
    message.digital_channels[1] = (byte & 0b010) != 0;
    message.failsafe = (byte & 0b100) != 0;
}



#[cfg(test)]
mod tests {
    use pretty_assertions::{assert_eq};
    use super::*;

    use heapless::spsc::Queue;

    #[test]
    fn sbus_decoder_decodess_single_valid_input() {
        let mut byte_queue = Queue::new();
        let (mut byte_producer, byte_consumer) = byte_queue.split();

        let mut message_queue = Queue::new();
        let (message_producer, mut message_consumer) = message_queue.split();
        let mut decoder = SbusDecoder::<()>::new(byte_consumer, message_producer);

        // Alternating max value, min value for each channel.
        // Bool channels == 1, failsafe == 0
        let bytes: [u8; 25] = [
            0x0f,
            0b1111_1110,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b1111_1111,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b0000_0011,
            0b0000_0000
        ];

        for byte in &bytes {
            byte_producer.enqueue(Ok(*byte)).unwrap();
        }

        decoder.process().unwrap();

        let decoded = message_consumer.dequeue();

        let expected = SbusMessage {
            channels: [
                0b111_1111_1110,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
            ],
            digital_channels: [true, true],
            failsafe: false
        };
        assert_eq!(decoded
                   .expect("Expected a message")
                   .expect("Expected message not to be Err")
                , expected
            );
    }

    #[test]
    fn sbus_decoder_detects_incorrect_stop_byte() {
        let mut byte_queue = Queue::new();
        let (mut byte_producer, byte_consumer) = byte_queue.split();

        let mut message_queue = Queue::new();
        let (message_producer, mut message_consumer) = message_queue.split();
        let mut decoder = SbusDecoder::<()>::new(byte_consumer, message_producer);

        byte_producer.enqueue(Ok(0x0f)).expect("Failed to enqueue header");
        for _ in 0..24 {
            byte_producer.enqueue(Ok(0x01)).expect("Failed to enqueue byte");
        }

        decoder.process().unwrap();

        let decoded = message_consumer.dequeue();
        assert_eq!(decoded, Some(Err(Error::MissingStopByte)));

        // Enqueue some simulated bytes
        byte_producer.enqueue(Ok(0x0f)).expect("failed to enqueue intermediate byte");
        byte_producer.enqueue(Ok(1)).expect("failed to enqueue intermediate byte");
        byte_producer.enqueue(Ok(1)).expect("failed to enqueue intermediate byte");

        // Enqueue a simulated stop byte
        byte_producer.enqueue(Ok(0)).expect("failed to enqueue simulated stop byte");

        decoder.process().unwrap();

        // Enqueue a full valid message


        let bytes: [u8; 25] = [
            0x0f,
            0b1111_1110,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b1111_1111,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b0000_0011,
            0b0000_0000
        ];

        for byte in &bytes {
            byte_producer.enqueue(Ok(*byte)).unwrap();
        }

        decoder.process().unwrap();

        let decoded = message_consumer.dequeue();

        let expected = SbusMessage {
            channels: [
                0b111_1111_1110,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
                0b111_1111_1111,
                0,
            ],
            digital_channels: [true, true],
            failsafe: false
        };
        assert_eq!(decoded
                   .expect("Expected a message")
                   .expect("Expected message not to be Err")
                , expected
            );
    }

    #[test]
    fn sbus_decoder_detects_failsafes() {
        let mut byte_queue = Queue::new();
        let (mut byte_producer, byte_consumer) = byte_queue.split();

        let mut message_queue = Queue::new();
        let (message_producer, mut message_consumer) = message_queue.split();
        let mut decoder = SbusDecoder::<()>::new(byte_consumer, message_producer);

        // Alternating max value, min value for each channel.
        // Bool channels == 1, failsafe == 0
        let bytes: [u8; 25] = [
            0x0f,
            0b1111_1110,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b1111_1111,
            0b0000_0111,
            0b1100_0000,
            0b1111_1111,
            0b0000_0001,
            0b1111_0000,
            0b0111_1111,
            0b0000_0000,
            0b1111_1100,
            0b0001_1111,
            0b0000_0000,
            0b0000_0111,
            0b0000_0000
        ];

        for byte in &bytes {
            byte_producer.enqueue(Ok(*byte)).unwrap();
        }

        decoder.process().unwrap();

        let decoded = message_consumer.dequeue();
        assert_eq!(decoded, Some(Err(Error::Failsafe)));
    }
}
