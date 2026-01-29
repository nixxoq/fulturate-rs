use teloxide::types::MessageEntityKind;
use teloxide::utils::html::{escape, link};

pub fn message_to_html(text: &str, entities: &[teloxide::types::MessageEntity]) -> String {
    let utf16_text: Vec<u16> = text.encode_utf16().collect();
    let mut html_result = String::new();
    let mut last_offset = 0;

    for entity in entities {
        if entity.offset > last_offset {
            let piece = String::from_utf16_lossy(&utf16_text[last_offset..entity.offset]);
            html_result.push_str(&escape(&piece));
        }

        let start = entity.offset;
        let end = entity.offset + entity.length;
        let piece = String::from_utf16_lossy(&utf16_text[start..end]);

        let formatted = match &entity.kind {
            MessageEntityKind::Bold => format!("<b>{}</b>", escape(&piece)),
            MessageEntityKind::Italic => format!("<i>{}</i>", escape(&piece)),
            MessageEntityKind::Code => format!("<code>{}</code>", escape(&piece)),
            MessageEntityKind::Pre { language } => match language {
                Some(lang) => format!(
                    "<pre><code class=\"language-{}\">{}</code></pre>",
                    lang,
                    escape(&piece)
                ),
                None => format!("<pre>{}</pre>", escape(&piece)),
            },
            MessageEntityKind::TextLink { url } => link(url.as_str(), &piece),
            MessageEntityKind::Underline => format!("<u>{}</u>", escape(&piece)),
            MessageEntityKind::Strikethrough => format!("<s>{}</u>", escape(&piece)),
            MessageEntityKind::Blockquote => format!("<blockquote>{}</blockquote>", escape(&piece)),
            MessageEntityKind::Spoiler => format!("<tg-spoiler>{}</tg-spoiler>", escape(&piece)),
            _ => escape(&piece),
        };

        html_result.push_str(&formatted);
        last_offset = end;
    }

    if last_offset < utf16_text.len() {
        let piece = String::from_utf16_lossy(&utf16_text[last_offset..]);
        html_result.push_str(&escape(&piece));
    }

    html_result
}
