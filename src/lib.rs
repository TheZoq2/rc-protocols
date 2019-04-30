#![no_std]

pub mod sbus;
pub mod cppm;
pub mod smart_port;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
