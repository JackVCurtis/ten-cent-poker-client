use ark_ff::Field;
use ark_serialize::CanonicalSerialize;
use merlin::Transcript;

#[allow(dead_code)] // currently only consumed by the not-yet-ported arguments layer
pub(crate) trait TranscriptProtocol {
    fn append(&mut self, label: &'static [u8], item: &impl CanonicalSerialize);

    fn challenge_scalar<F: Field>(&mut self, label: &'static [u8]) -> F;
}

impl TranscriptProtocol for Transcript {
    fn append(&mut self, label: &'static [u8], item: &impl CanonicalSerialize) {
        let mut bytes = Vec::new();
        // 0.3 `serialize` == 0.6 `serialize_compressed`.
        item.serialize_compressed(&mut bytes).unwrap();
        self.append_message(label, &bytes)
    }

    fn challenge_scalar<F>(&mut self, label: &'static [u8]) -> F
    where
        F: Field,
    {
        let example = F::one();
        // 0.3 `serialized_size()` == 0.6 `compressed_size()`.
        let size = example.compressed_size();
        let mut buf = vec![0u8; size];
        self.challenge_bytes(label, &mut buf);
        F::from_random_bytes(&buf).unwrap()
    }
}

#[cfg(test)]
mod transcript_test {
    use crate::curve::Fr;
    use ark_serialize::CanonicalSerialize;

    #[test]
    fn f_size() {
        use ark_ff::One;
        let one = Fr::one();
        let serialized_size = one.compressed_size();
        let uncompressed_size = one.uncompressed_size();

        // expect compressed&uncompressed size to be the same for the field
        assert_eq!(serialized_size, uncompressed_size);
    }
}
