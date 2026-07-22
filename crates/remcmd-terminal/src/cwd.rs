const MAX_OSC_PAYLOAD_BYTES: usize = 4096;

#[derive(Default)]
pub(crate) struct Osc7Parser {
    state: ParserState,
    payload: Vec<u8>,
    overflowed: bool,
}

pub(crate) struct Osc7Event {
    pub(crate) end_offset: usize,
    pub(crate) path: String,
}

#[derive(Clone, Copy, Default)]
enum ParserState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

impl Osc7Parser {
    pub(crate) fn advance(&mut self, bytes: &[u8]) -> Vec<Osc7Event> {
        let mut events = Vec::new();

        for (offset, &byte) in bytes.iter().enumerate() {
            match self.state {
                ParserState::Ground => match byte {
                    0x1b => self.state = ParserState::Escape,
                    0x9d => self.start_osc(),
                    _ => {}
                },
                ParserState::Escape => match byte {
                    b']' => self.start_osc(),
                    0x1b => {}
                    _ => self.state = ParserState::Ground,
                },
                ParserState::Osc => match byte {
                    0x07 | 0x9c => self.finish_osc(offset + 1, &mut events),
                    0x1b => self.state = ParserState::OscEscape,
                    _ => self.push_payload(byte),
                },
                ParserState::OscEscape => {
                    if byte == b'\\' {
                        self.finish_osc(offset + 1, &mut events);
                    } else {
                        self.push_payload(0x1b);
                        self.push_payload(byte);
                        self.state = ParserState::Osc;
                    }
                }
            }
        }

        events
    }

    fn start_osc(&mut self) {
        self.payload.clear();
        self.overflowed = false;
        self.state = ParserState::Osc;
    }

    fn push_payload(&mut self, byte: u8) {
        if self.payload.len() < MAX_OSC_PAYLOAD_BYTES {
            self.payload.push(byte);
        } else {
            self.overflowed = true;
        }
    }

    fn finish_osc(&mut self, end_offset: usize, events: &mut Vec<Osc7Event>) {
        if !self.overflowed
            && let Some(path) = parse_osc7_path(&self.payload)
        {
            events.push(Osc7Event { end_offset, path });
        }
        self.payload.clear();
        self.overflowed = false;
        self.state = ParserState::Ground;
    }
}

fn parse_osc7_path(payload: &[u8]) -> Option<String> {
    let payload = std::str::from_utf8(payload).ok()?.strip_prefix("7;")?;
    let location = payload
        .strip_prefix("file://")
        .or_else(|| payload.strip_prefix("kitty-shell-cwd://"))?;
    let path = &location[location.find('/')?..];
    let path = percent_decode(path.as_bytes())?;
    let path = String::from_utf8(path).ok()?;

    (!path.is_empty() && path.starts_with('/') && !path.chars().any(char::is_control))
        .then_some(path)
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;

    while index < input.len() {
        if input[index] == b'%' {
            let high = *input.get(index + 1)?;
            let low = *input.get(index + 2)?;
            decoded.push(hex_value(high)? << 4 | hex_value(low)?);
            index += 3;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }

    Some(decoded)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(events: Vec<Osc7Event>) -> Vec<String> {
        events.into_iter().map(|event| event.path).collect()
    }

    #[test]
    fn parses_bel_and_st_terminated_cwd_sequences() {
        let mut parser = Osc7Parser::default();

        assert_eq!(
            paths(parser.advance(b"\x1b]7;file://server/home/test%20user\x07")),
            vec!["/home/test user"]
        );
        assert_eq!(
            paths(parser.advance(b"\x1b]7;kitty-shell-cwd://server/var/log\x1b\\")),
            vec!["/var/log"]
        );
    }

    #[test]
    fn preserves_a_sequence_split_across_output_chunks() {
        let mut parser = Osc7Parser::default();

        assert!(
            parser
                .advance(b"prompt\x1b]7;file://server/home/")
                .is_empty()
        );
        assert_eq!(paths(parser.advance(b"test\x07$ ")), vec!["/home/test"]);
    }

    #[test]
    fn ignores_non_cwd_and_invalid_path_sequences() {
        let mut parser = Osc7Parser::default();

        assert!(parser.advance(b"\x1b]2;window title\x07").is_empty());
        assert!(
            parser
                .advance(b"\x1b]7;file://server/invalid%xx\x07")
                .is_empty()
        );
        assert!(
            parser
                .advance(b"\x1b]7;https://server/home/test\x07")
                .is_empty()
        );
    }
}
