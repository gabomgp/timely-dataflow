use serialization::Serializable;
use abomonation::{Abomonation, encode, decode};
use columnar::Columnar;

impl<T: Abomonation+Columnar+Clone+Eq> Serializable for T {
    fn encode(typed: Self, bytes: &mut Vec<u8>) {
        encode(&vec![typed], bytes);
    }
    fn decode(bytes: &mut [u8]) -> Result<Self, &mut [u8]> {
        let result = if let Ok(result) = decode::<T>(bytes) { Ok(result[0].clone()) } else { Err(()) };
        if let Ok(result) = result { Ok(result) } else { Err (bytes) }
    }
}