//! How messages are cut out of a byte stream, the way LSP does it: a `Content-Length: N` header
//! line, other header lines ignored, an empty line, then N bytes of JSON. Lines end in `\r\n`; a
//! bare `\n` is read too.

use std::io::{self, BufRead, Write};

/// No message is bigger: a stream that says otherwise is broken, not a message to wait for.
pub const MAX_MESSAGE: usize = 64 << 20;

pub fn write(w: &mut impl Write, message: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", message.len())?;
    w.write_all(message)?;
    w.flush()
}

/// The next message, or `None` when the stream ended cleanly between messages.
pub fn read(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            return match first {
                true => Ok(None),
                false => Err(bad("the stream ended inside a header")),
            };
        }
        first = false;
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        if text.is_empty() {
            break;
        }
        let text = std::str::from_utf8(text).map_err(|_| bad("a header is not UTF-8"))?;
        let header = text.split_once(':').filter(|(name, _)| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        let Some((name, value)) = header else {
            return Err(bad(&format!("{text:?} is not a header")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = (value.trim().parse()).map_err(|_| {
                bad(&format!(
                    "Content-Length {:?} is not a number",
                    value.trim()
                ))
            })?;
            if n > MAX_MESSAGE {
                return Err(bad(&format!("a message of {n} bytes is too big")));
            }
            length = Some(n);
        }
    }
    let n = length.ok_or_else(|| bad("a message without Content-Length"))?;
    let mut body = vec![0; n];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Read};

    fn framed(messages: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for m in messages {
            write(&mut out, m.as_bytes()).unwrap();
        }
        out
    }

    fn all(bytes: impl Read) -> Vec<String> {
        let mut r = BufReader::new(bytes);
        let mut out = Vec::new();
        while let Some(m) = read(&mut r).unwrap() {
            out.push(String::from_utf8(m).unwrap());
        }
        out
    }

    /// Hands out one byte per read, as a pipe may.
    struct Trickle(Vec<u8>, usize);

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(b) = self.0.get(self.1) else {
                return Ok(0);
            };
            buf[0] = *b;
            self.1 += 1;
            Ok(1)
        }
    }

    #[test]
    fn writes_lsp_headers() {
        assert_eq!(framed(&["{}"]), b"Content-Length: 2\r\n\r\n{}");
    }

    #[test]
    fn reads_several_messages_and_partial_reads() {
        let messages = ["{\"a\":1}", "{\"text\":\"ó🌋\"}", ""];
        assert_eq!(all(&framed(&messages)[..]), messages);
        assert_eq!(all(Trickle(framed(&messages), 0)), messages);
    }

    #[test]
    fn reads_other_headers_and_bare_newlines() {
        let input = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\
                     content-length:  3\r\n\r\n[1]Content-Length: 2\n\n{}";
        assert_eq!(all(input.as_bytes()), ["[1]", "{}"]);
    }

    #[test]
    fn refuses_bad_input() {
        let error = |input: &[u8]| {
            let mut r = BufReader::new(input);
            read(&mut r).unwrap_err().to_string()
        };
        assert!(error(b"{\"jsonrpc\": \"2.0\"}\n").contains("not a header"));
        assert!(error(b"Content-Type: x\r\n\r\n{}").contains("without Content-Length"));
        assert!(error(b"Content-Length: two\r\n\r\n").contains("not a number"));
        assert!(error(b"Content-Length: 99999999999\r\n\r\n").contains("too big"));
        assert!(
            error(b"Content-Length: 5\r\n\r\n{}").contains("fill"),
            "cut short"
        );
        assert!(error(b"Content-Length: 5\r\n").contains("inside a header"));
    }
}
