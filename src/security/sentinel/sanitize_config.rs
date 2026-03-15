/// Configuration for the Sentinel unicode sanitization pipeline.
/// Controls which sanitization steps are applied to inbound messages.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct SanitizationConfig {
    /// Strip zero-width characters (U+200B, U+200C, U+FEFF, U+00AD, U+034F, U+180E, U+FFFC).
    pub strip_zero_width: bool,
    /// Strip tag characters (U+E0001–U+E007F).
    pub strip_tag_characters: bool,
    /// Apply NFKC normalization (collapses homoglyphs). Allocates for non-ASCII text.
    pub normalize_unicode: bool,
    /// Strip bidi override control characters (U+202E, U+202D) and line/paragraph separators.
    pub strip_bidi_overrides: bool,
    /// Preserve U+200D (ZWJ) when it appears between emoji codepoints.
    pub preserve_emoji_zwj: bool,
    /// Also sanitize metadata fields (sender, reply_target) not just message body.
    pub sanitize_metadata_fields: bool,
    /// Log when sanitization modifies a message.
    pub log_sanitizations: bool,
}

impl Default for SanitizationConfig {
    fn default() -> Self {
        Self {
            strip_zero_width: true,
            strip_tag_characters: true,
            normalize_unicode: true,
            strip_bidi_overrides: true,
            preserve_emoji_zwj: true,
            sanitize_metadata_fields: true,
            log_sanitizations: true,
        }
    }
}
