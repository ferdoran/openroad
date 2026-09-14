// The per-frame sequence byte.
//
// A small bit-mangling generator seeded from the handshake; it carries no key
// material of its own. The construction is a fact about the wire format, taken
// from our own capture corpus and cross-checked against the fan descriptions
// whose shared provenance is documented in `security.rs`.
#[derive(Copy, Clone)]
pub(crate) struct Sequence {
    b0: u8,
    b1: u8,
    b2: u8,
}

impl From<u32> for Sequence {
    fn from(value: u32) -> Self {
        let round1 = generate_value(value);
        let round2 = generate_value(round1);
        let round3 = generate_value(round2);
        let round4 = generate_value(round3);
        let mut b1 = ((round1 & 0xFF) ^ (round2 & 0xFF)) as u8;
        let mut b2 = ((round4 & 0xFF) ^ (round3 & 0xFF)) as u8;
        if b1 == 0 {
            b1 = 1;
        }

        if b2 == 0 {
            b2 = 1;
        }
        let b0 = (b2 ^ b1) as u8;

        Self { b0, b1, b2 }
    }
}

impl Sequence {
    pub(crate) fn next(&mut self) -> u8 {
        let result = (self.b2 as u32 * ((!self.b0) as u32 + self.b1 as u32) as u32) as u8;
        let result = (result ^ (result >> 4)) as u8;
        self.b0 = result;
        result
    }
}

fn generate_value(value: u32) -> u32 {
    let mut new_val = value;
    let complement = !1_u32;

    for _ in 0..32 {
        let mut v = new_val;
        v = (v >> 2) ^ new_val;
        v = (v >> 2) ^ new_val;
        v = (v >> 1) ^ new_val;
        v = (v >> 1) ^ new_val;
        v = (v >> 1) ^ new_val;
        new_val = ((new_val >> 1) | (new_val << 31)) & complement | (v & 1);
    }

    return new_val;
}
