use unicode_segmentation::UnicodeSegmentation;

pub(crate) fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for character in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += character.len_utf16();
        utf8_offset += character.len_utf8();
    }
    utf8_offset
}

pub(crate) fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;
    for character in text.chars() {
        if utf8_count >= offset {
            break;
        }
        utf8_count += character.len_utf8();
        utf16_offset += character.len_utf16();
    }
    utf16_offset
}

pub(crate) fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

pub(crate) fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_offsets_round_trip_unicode_boundaries_and_clamp_past_the_end() {
        let text = "a😀中e\u{301}";
        for offset in text
            .char_indices()
            .map(|(index, _)| index)
            .chain([text.len()])
        {
            assert_eq!(
                offset_from_utf16(text, offset_to_utf16(text, offset)),
                offset
            );
        }
        // Preserve the input controls' forward snapping for half a surrogate pair.
        assert_eq!(offset_from_utf16(text, 2), 5);
        assert_eq!(offset_to_utf16(text, 2), 3);
        assert_eq!(offset_from_utf16(text, usize::MAX), text.len());
        assert_eq!(
            offset_to_utf16(text, usize::MAX),
            text.encode_utf16().count()
        );
    }

    #[test]
    fn cursor_movement_keeps_combining_characters_and_emoji_clusters_together() {
        let text = "a👩‍💻e\u{301}\r\n";
        let boundaries = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .chain([text.len()])
            .collect::<Vec<_>>();
        for pair in boundaries.windows(2) {
            assert_eq!(next_boundary(text, pair[0]), pair[1]);
            assert_eq!(previous_boundary(text, pair[1]), pair[0]);
        }
        assert_eq!(previous_boundary(text, 0), 0);
        assert_eq!(next_boundary(text, text.len()), text.len());
        assert_eq!(next_boundary("", 0), 0);
        assert_eq!(previous_boundary("", 0), 0);
    }
}
