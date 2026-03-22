#[allow(dead_code)]
pub struct RingBuffer {
    buf: Box<[u8]>,
    write_pos: usize,
    len: usize,
}

#[allow(dead_code)]
impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0u8; capacity].into_boxed_slice(),
            write_pos: 0,
            len: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        let cap = self.buf.len();
        if cap == 0 {
            return;
        }

        // If data is larger than capacity, only keep the tail
        let data = if data.len() > cap {
            &data[data.len() - cap..]
        } else {
            data
        };

        let first_chunk = cap - self.write_pos;
        if data.len() <= first_chunk {
            self.buf[self.write_pos..self.write_pos + data.len()].copy_from_slice(data);
        } else {
            self.buf[self.write_pos..self.write_pos + first_chunk]
                .copy_from_slice(&data[..first_chunk]);
            let remainder = data.len() - first_chunk;
            self.buf[..remainder].copy_from_slice(&data[first_chunk..]);
        }

        self.write_pos = (self.write_pos + data.len()) % cap;
        self.len = (self.len + data.len()).min(cap);
    }

    pub fn as_slices(&self) -> (&[u8], &[u8]) {
        if self.len == 0 {
            return (&[], &[]);
        }
        if self.len < self.buf.len() {
            // Buffer hasn't wrapped yet
            let start = if self.write_pos >= self.len {
                self.write_pos - self.len
            } else {
                self.buf.len() - (self.len - self.write_pos)
            };
            if start < self.write_pos {
                (&self.buf[start..self.write_pos], &[])
            } else {
                (&self.buf[start..], &self.buf[..self.write_pos])
            }
        } else {
            // Buffer is full
            (&self.buf[self.write_pos..], &self.buf[..self.write_pos])
        }
    }

    pub fn last_n_lines(&self, n: usize) -> Vec<u8> {
        if n == 0 || self.len == 0 {
            return Vec::new();
        }

        let (first, second) = self.as_slices();
        let mut result = Vec::with_capacity(first.len() + second.len());
        result.extend_from_slice(first);
        result.extend_from_slice(second);

        // Scan backwards for newlines
        let mut lines_found = 0;
        let mut cut_pos = 0;
        for (i, &b) in result.iter().enumerate().rev() {
            if b == b'\n' {
                lines_found += 1;
                if lines_found == n + 1 {
                    cut_pos = i + 1;
                    break;
                }
            }
        }

        result[cut_pos..].to_vec()
    }

    pub fn all_bytes(&self) -> Vec<u8> {
        let (first, second) = self.as_slices();
        let mut result = Vec::with_capacity(first.len() + second.len());
        result.extend_from_slice(first);
        result.extend_from_slice(second);
        result
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.write_pos = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let mut rb = RingBuffer::new(16);
        rb.write(b"hello");
        assert_eq!(rb.len(), 5);
        assert_eq!(rb.all_bytes(), b"hello");
    }

    #[test]
    fn test_wrap_around() {
        let mut rb = RingBuffer::new(8);
        rb.write(b"12345");
        rb.write(b"67890");
        assert_eq!(rb.len(), 8);
        assert_eq!(rb.all_bytes(), b"34567890");
    }

    #[test]
    fn test_data_larger_than_capacity() {
        let mut rb = RingBuffer::new(4);
        rb.write(b"abcdefgh");
        assert_eq!(rb.len(), 4);
        assert_eq!(rb.all_bytes(), b"efgh");
    }

    #[test]
    fn test_last_n_lines() {
        let mut rb = RingBuffer::new(64);
        rb.write(b"line1\nline2\nline3\nline4\n");
        let last2 = rb.last_n_lines(2);
        assert_eq!(last2, b"line3\nline4\n");
    }

    #[test]
    fn test_empty() {
        let rb = RingBuffer::new(16);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.all_bytes(), b"");
    }
}
