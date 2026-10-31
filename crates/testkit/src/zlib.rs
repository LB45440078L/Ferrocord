//! Server side of Discord's `compress=zlib-stream` transport: one deflate stream
//! per connection, every message flushed with `Z_SYNC_FLUSH` so it ends in
//! `00 00 FF FF`.

use flate2::{Compress, Compression, FlushCompress};

/// Marker that terminates every zlib-stream message.
pub const SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// A connection-long zlib compressor.
pub struct ZlibStreamEncoder {
    c: Compress,
}

impl Default for ZlibStreamEncoder {
    fn default() -> Self {
        Self { c: Compress::new(Compression::default(), true) }
    }
}

impl ZlibStreamEncoder {
    /// Compresses one message and sync-flushes it.
    pub fn encode(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() / 2 + 64);
        let start = self.c.total_in();
        loop {
            if out.capacity() - out.len() < 64 {
                out.reserve(out.capacity().max(1024));
            }
            let consumed = (self.c.total_in() - start) as usize;
            self.c.compress_vec(&data[consumed..], &mut out, FlushCompress::Sync).expect("deflate");
            let consumed = (self.c.total_in() - start) as usize;
            // The flush is complete once all input is consumed and deflate left output space unused.
            if consumed == data.len() && out.len() < out.capacity() {
                break;
            }
        }
        debug_assert!(out.ends_with(&SUFFIX), "sync flush must end with 00 00 ff ff");
        out
    }
}

/// Splits a compressed message into websocket frames of about `chunk` bytes.
///
/// A chunk boundary is moved forward whenever the prefix would itself end with
/// the sync-flush marker, because a client would (correctly) treat that as a
/// complete message.
pub fn split_frames(data: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    let chunk = chunk.max(1);
    let mut out = vec![];
    let mut start = 0;
    while start < data.len() {
        let mut end = (start + chunk).min(data.len());
        while end < data.len() && data[..end].ends_with(&SUFFIX) {
            end += 1;
        }
        out.push(data[start..end].to_vec());
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Decompress, FlushDecompress};

    #[test]
    fn encodes_a_stream() {
        let mut enc = ZlibStreamEncoder::default();
        let mut dec = Decompress::new(true);
        for msg in [b"{\"op\":10}".to_vec(), vec![b'x'; 200_000], b"{\"op\":11}".to_vec()] {
            let z = enc.encode(&msg);
            assert!(z.ends_with(&SUFFIX));
            let mut out = Vec::with_capacity(msg.len() + 64);
            dec.decompress_vec(&z, &mut out, FlushDecompress::Sync).unwrap();
            assert_eq!(out, msg);
            let parts = split_frames(&z, 7);
            assert_eq!(parts.concat(), z);
            for p in &parts[..parts.len() - 1] {
                assert!(!p.ends_with(&SUFFIX));
            }
        }
    }
}
