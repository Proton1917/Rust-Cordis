use crate::{CodecError, Limits, Message};
use parking_lot::Mutex;
use std::{
    io::{self, Write},
    sync::Arc,
};

pub(super) struct Budget {
    maximum: usize,
    reserve: usize,
    usage: Mutex<Usage>,
}
#[derive(Default)]
struct Usage {
    data: usize,
    control: usize,
    peak: usize,
}

impl Budget {
    pub(super) fn new(maximum: usize, reserve: usize) -> Arc<Self> {
        Arc::new(Self {
            maximum,
            reserve,
            usage: Mutex::new(Usage::default()),
        })
    }
    pub(super) fn bytes(&self) -> usize {
        let usage = self.usage.lock();
        usage.data + usage.control
    }
    pub(super) fn peak(&self) -> usize {
        self.usage.lock().peak
    }
    pub(super) fn encode(
        self: &Arc<Self>,
        message: &Message,
        limits: &Limits,
        control: bool,
    ) -> Result<Encoded, Failure> {
        let mut output = Buffer {
            bytes: Vec::new(),
            lease: Lease {
                budget: self.clone(),
                data: 0,
                control: 0,
                privileged: control,
            },
            maximum: limits.max_frame.saturating_add(4),
            exhausted: false,
        };
        if let Err(problem) = crate::codec::encode_frame_to(message, limits, &mut output) {
            return Err(Failure {
                problem,
                exhausted: output.exhausted,
            });
        }
        let length = (output.bytes.len() - 4) as u32;
        output.bytes[..4].copy_from_slice(&length.to_be_bytes());
        Ok(Encoded {
            bytes: output.bytes,
            lease: output.lease,
            control,
        })
    }
}

pub(super) struct Failure {
    pub(super) problem: CodecError,
    pub(super) exhausted: bool,
}

pub(super) struct Encoded {
    pub(super) bytes: Vec<u8>,
    pub(super) lease: Lease,
    pub(super) control: bool,
}

pub(super) struct Lease {
    budget: Arc<Budget>,
    data: usize,
    control: usize,
    privileged: bool,
}
impl Lease {
    fn capacity(&self) -> usize {
        self.data + self.control
    }
    fn grow(&mut self, capacity: usize) -> bool {
        let needed = capacity.saturating_sub(self.capacity());
        if needed == 0 {
            return true;
        }
        let mut usage = self.budget.usage.lock();
        let data = needed.min(self.budget.maximum - usage.data);
        let control = needed - data;
        if control > 0 && (!self.privileged || control > self.budget.reserve - usage.control) {
            return false;
        }
        usage.data += data;
        usage.control += control;
        usage.peak = usage.peak.max(usage.data + usage.control);
        self.data += data;
        self.control += control;
        true
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock();
        usage.data -= self.data;
        usage.control -= self.control;
    }
}

struct Buffer {
    // Release the allocation before publishing its capacity back to the budget.
    bytes: Vec<u8>,
    lease: Lease,
    maximum: usize,
    exhausted: bool,
}
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let needed = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("frame size overflow"))?;
        if needed > self.bytes.capacity() {
            let target = needed
                .checked_next_power_of_two()
                .unwrap_or(needed)
                .max(128)
                .min(self.maximum);
            if !self.lease.grow(target) && !self.lease.grow(needed) {
                self.exhausted = true;
                return Err(io::Error::other(
                    "connection buffered-byte budget exhausted",
                ));
            }
            self.bytes
                .try_reserve_exact(self.lease.capacity() - self.bytes.len())
                .map_err(|problem| {
                    self.exhausted = true;
                    io::Error::other(problem)
                })?;
            // Allocator implementations may round a requested capacity upward.
            if !self.lease.grow(self.bytes.capacity()) {
                self.exhausted = true;
                return Err(io::Error::other(
                    "connection buffer allocation exceeds its byte budget",
                ));
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
