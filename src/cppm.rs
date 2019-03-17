use embedded_hal::{digital::OutputPin, timer::CountDown};
use core::marker::PhantomData;

// All timings are expressed in microseconds
const FRAME_TIME: u32 = 22_000;
const MIN_TIME: u32 = 690;
const MAX_TIME: u32 = 1710;
const SEPARATION: u32 = 300;
const CHANNEL_COUNT: usize = 8;

#[derive(Clone)]
pub struct PpmFrame<TIME> {
    signal_timings: [TIME; CHANNEL_COUNT],
    frame_padding: TIME,
    _timer: PhantomData<TIME>
}

impl<TIME> PpmFrame<TIME>
    where TIME: Clone
{
    pub fn from_channels(
        channels: [f32; CHANNEL_COUNT],
        us_to_time: impl Fn(u32) -> TIME
    ) -> Self {
        let dummy_time = us_to_time(1);
        let mut signal_timings = [
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
            dummy_time.clone(),
        ];
        let mut total_time = 0;
        for (i, channel) in channels.iter().enumerate() {
            let time = MIN_TIME + ((MAX_TIME - MIN_TIME) as f32 * channel) as u32;

            signal_timings[i] = us_to_time(time);

            total_time += SEPARATION + time;
        }

        Self {
            signal_timings,
            frame_padding: us_to_time(FRAME_TIME - total_time),
            _timer: PhantomData,
        }
    }
}

pub struct CppmWriter<PIN, TIMER>
where
    PIN: OutputPin,
    TIMER: CountDown
{
    pin: PIN,
    // Index of the next signal to send
    index: usize,
    is_low: bool,
    current_frame: PpmFrame<TIMER::Time>,
    time_300us: TIMER::Time
}

impl<PIN, TIMER> CppmWriter<PIN, TIMER>
where
    PIN: OutputPin,
    TIMER: CountDown,
    TIMER::Time: Copy
{
    pub fn new(
        mut pin: PIN,
        initial_frame: PpmFrame<TIMER::Time>,
        time_300us: TIMER::Time
    ) -> Self {
        pin.set_high();

        Self {
            pin,
            index: 0,
            is_low: false,
            current_frame: initial_frame,
            time_300us
        }
    }

    /// Timer event handler.
    pub fn on_timer(
        &mut self,
        timer: &mut TIMER,
        next_frame: &PpmFrame<TIMER::Time>
    ) {
        if self.is_low {
            self.pin.set_high();
            if self.index == CHANNEL_COUNT {
                timer.start(self.current_frame.frame_padding);

                self.current_frame = next_frame.clone();

                self.index = 0;
            }
            else {
                timer.start(self.current_frame.signal_timings[self.index]);
                self.index += 1
            }
            self.is_low = false;
        }
        else {
            timer.start(self.time_300us);
            self.pin.set_low();
            self.is_low = true;
        }
    }
}
