//! `compress=zlib-stream` transport: one zlib context for the lifetime of the
//! connection; each message ends with the `Z_SYNC_FLUSH` marker `00 00 FF FF`.

use flate2::{Decompress, FlushDecompress, Status};

const SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

pub struct ZlibStream {
    inflater: Decompress,
    buffer: Vec<u8>,
    out: Vec<u8>,
}

impl Default for ZlibStream {
    fn default() -> Self {
        Self { inflater: Decompress::new(true), buffer: Vec::with_capacity(64 * 1024), out: Vec::with_capacity(256 * 1024) }
    }
}

impl ZlibStream {
    /// Feeds a binary frame. Returns the decompressed message once a full one is buffered.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<&[u8]>, flate2::DecompressError> {
        self.buffer.extend_from_slice(frame);
        if self.buffer.len() < 4 || self.buffer[self.buffer.len() - 4..] != SUFFIX {
            return Ok(None);
        }
        self.out.clear();
        let mut consumed = 0usize;
        loop {
            if self.out.capacity() - self.out.len() < 32 * 1024 {
                self.out.reserve(self.out.capacity().max(64 * 1024));
            }
            let before_in = self.inflater.total_in();
            let status = self.inflater.decompress_vec(&self.buffer[consumed..], &mut self.out, FlushDecompress::Sync)?;
            consumed += (self.inflater.total_in() - before_in) as usize;
            let spare = self.out.capacity() - self.out.len();
            if consumed >= self.buffer.len() && spare > 0 {
                break;
            }
            if status == Status::StreamEnd {
                break;
            }
            if status == Status::BufError && consumed >= self.buffer.len() {
                break;
            }
        }
        self.buffer.clear();
        // Release memory after a very large READY.
        if self.buffer.capacity() > 4 * 1024 * 1024 {
            self.buffer.shrink_to(64 * 1024);
        }
        Ok(Some(&self.out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    #[test]
    fn stream_roundtrip() {
        let mut c = Compress::new(Compression::default(), true);
        let mut z = ZlibStream::default();
        for msg in [&b"{\"op\":10}"[..], &b"{\"op\":11,\"d\":null}"[..], &vec![b'a'; 300_000][..]] {
            let mut out = Vec::with_capacity(msg.len() + 1024);
            c.compress_vec(msg, &mut out, FlushCompress::Sync).unwrap();
            // Split into two frames to test buffering.
            let (a, b) = out.split_at(out.len() / 2);
            assert!(z.push(a).unwrap().is_none() || b.is_empty());
            let got = z.push(b).unwrap().unwrap().to_vec();
            assert_eq!(got, msg);
        }
    }
}
