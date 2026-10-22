/// Incremental Server-Sent Events parser. Feed raw bytes as they arrive;
/// events come out once their terminating blank line has been seen.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, sep_len)) = find_blank_line(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + sep_len).collect();
            let text = String::from_utf8_lossy(&block[..end]);
            if let Some(ev) = parse_block(&text) {
                out.push(ev);
            }
        }
        out
    }
}

fn find_blank_line(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn parse_block(text: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(SseEvent { event, data: data.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: a\ndata: {\"x\"").is_empty());
        let evs = p.feed(b":1}\n\ndata: second\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], SseEvent { event: Some("a".into()), data: "{\"x\":1}".into() });
        assert_eq!(evs[1].data, "second");
    }

    #[test]
    fn handles_crlf_comments_and_multiline_data() {
        let mut p = SseParser::default();
        let evs = p.feed(b": keepalive\r\n\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "a\nb");
    }
}
