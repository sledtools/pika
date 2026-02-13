#[derive(Debug, Clone, Default)]
pub struct OpusCodec;

impl OpusCodec {
    pub fn encode_pcm_i16(&self, pcm: &[i16]) -> Vec<u8> {
        // Phase-1 wire format placeholder.
        pcm.iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<u8>>()
    }

    pub fn decode_to_pcm_i16(&self, packet: &[u8]) -> Vec<i16> {
        packet
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }
}
