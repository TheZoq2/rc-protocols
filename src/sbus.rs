use heapless::{Vec, spsc::{Producer, Consumer}};
use heapless::consts::*;

const HEADER_BYTE: u8 = 0x0f;
const FOOTER_BYTE: u8 = 0x00;

const CHANNEL_AMOUNT: u8 = 16;
const CHANNEL_BYTE_COUNT: usize = 22;

#[derive(Debug, PartialEq)]
pub enum Error {
    MissingStopByte
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

    pub fn process(&mut self) {
        loop {
            let mut _count = 0;
            let byte = match self.byte_rx.dequeue() {
                Some(byte) => byte.unwrap_or(0), // TODO: Handle this error
                None => break
            };

            let new_state = match self.state.clone() {
                DecoderState::WaitForHeader => {
                    if byte == HEADER_BYTE {
                        DecoderState::Channel(Vec::default())
                    }
                    else {
                        DecoderState::Recover
                    }
                }
                DecoderState::Channel(mut previous_bytes) => {
                    if previous_bytes.len() < CHANNEL_BYTE_COUNT {
                        previous_bytes.push(byte);
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
                        self.result_tx.enqueue(Ok(self.current_message.clone()));
                        DecoderState::WaitForHeader
                    }
                    else {
                        self.result_tx.enqueue(Err(Error::MissingStopByte));
                        DecoderState::Recover
                    }
                }
                DecoderState::Recover => {
                    if byte == FOOTER_BYTE {
                        DecoderState::WaitForHeader
                    }
                    else {
                        DecoderState::Recover
                    }
                }
            };

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

        decoder.process();

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

        decoder.process();

        let decoded = message_consumer.dequeue();
        assert_eq!(decoded, Some(Err(Error::MissingStopByte)));

        // Enqueue some simulated bytes
        byte_producer.enqueue(Ok(0x0f)).expect("failed to enqueue intermediate byte");
        byte_producer.enqueue(Ok(1)).expect("failed to enqueue intermediate byte");
        byte_producer.enqueue(Ok(1)).expect("failed to enqueue intermediate byte");

        // Enqueue a simulated stop byte
        byte_producer.enqueue(Ok(0)).expect("failed to enqueue simulated stop byte");

        decoder.process();

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

        decoder.process();

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
}
