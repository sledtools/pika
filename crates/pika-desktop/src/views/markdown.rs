//! Lightweight Markdown → Iced rendering for message bubbles.
//!
//! Converts a markdown string into an `Element` tree using `pulldown-cmark`
//! for parsing and Iced's `rich_text` / `span` for styled inline content.
//! Block-level elements (code blocks, blockquotes, lists) use `container`
//! and `column` layout.

use std::borrow::Cow;

use iced::advanced::text::Span;
use iced::widget::{column, container, rich_text, row, span, text, Space};
use iced::{border, font, Background, Color, Element, Font, Theme};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::conversation::Message;
use crate::theme;

/// Monospace font for inline code and code blocks.
const MONO_FONT: Font = Font::with_name("monospace");

/// Bold variant of the default font.
const BOLD_FONT: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

/// Italic variant of the default font.
const ITALIC_FONT: Font = Font {
    style: font::Style::Italic,
    ..Font::DEFAULT
};

/// Bold-italic variant of the default font.
const BOLD_ITALIC_FONT: Font = Font {
    weight: font::Weight::Bold,
    style: font::Style::Italic,
    ..Font::DEFAULT
};

/// Inline style state tracked while walking the event stream.
#[derive(Clone, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    link: bool,
}

impl InlineStyle {
    fn font(&self) -> Option<Font> {
        match (self.bold, self.italic) {
            (true, true) => Some(BOLD_ITALIC_FONT),
            (true, false) => Some(BOLD_FONT),
            (false, true) => Some(ITALIC_FONT),
            (false, false) => None,
        }
    }
}

/// Background colour for fenced / indented code blocks.
fn code_block_bg(is_mine: bool) -> Color {
    if is_mine {
        Color::WHITE.scale_alpha(0.10)
    } else {
        theme::text_primary().scale_alpha(0.06)
    }
}

/// Accent bar colour for blockquotes.
fn blockquote_bar(is_mine: bool) -> Color {
    if is_mine {
        Color::WHITE.scale_alpha(0.5)
    } else {
        theme::accent_blue().scale_alpha(0.5)
    }
}

/// Flush accumulated inline spans into a `rich_text` element and push it.
fn flush_spans<'a>(
    spans: &mut Vec<Span<'a, (), Font>>,
    target: &mut Vec<Element<'a, Message, Theme>>,
) {
    if !spans.is_empty() {
        target.push(rich_text(std::mem::take(spans)).size(15).into());
    }
}

/// Wrap a list of elements in a blockquote bar and return it.
fn wrap_blockquote<'a>(
    inner: Vec<Element<'a, Message, Theme>>,
    is_mine: bool,
) -> Element<'a, Message, Theme> {
    let bar_color = blockquote_bar(is_mine);
    row![
        container(Space::new().width(3).height(iced::Fill)).style(move |_: &Theme| {
            container::Style {
                background: Some(Background::Color(bar_color)),
                border: border::rounded(1),
                ..Default::default()
            }
        }),
        column(inner).spacing(2),
    ]
    .spacing(8)
    .into()
}

/// Render a markdown string as an Iced element.
///
/// `text_color` is the base text colour (white for sent, primary for received).
pub fn render_markdown<'a>(
    content: &'a str,
    text_color: Color,
    is_mine: bool,
) -> Element<'a, Message, Theme> {
    let opts = Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(content, opts);

    let mut blocks: Vec<Element<'a, Message, Theme>> = Vec::new();
    let mut inline_spans: Vec<Span<'a, (), Font>> = Vec::new();
    let mut style = InlineStyle::default();
    let mut in_code_block = false;
    let mut code_block_text = String::new();
    let mut list_index: Option<u64> = None;
    // Stack of blockquote element lists to support nesting.
    let mut bq_stack: Vec<Vec<Element<'a, Message, Theme>>> = Vec::new();

    /// Returns a mutable reference to the current output target: the top of
    /// the blockquote stack if inside a blockquote, otherwise `blocks`.
    fn target<'a, 'b>(
        blocks: &'b mut Vec<Element<'a, Message, Theme>>,
        bq_stack: &'b mut Vec<Vec<Element<'a, Message, Theme>>>,
    ) -> &'b mut Vec<Element<'a, Message, Theme>> {
        bq_stack.last_mut().unwrap_or(blocks)
    }

    for event in parser {
        match event {
            // ── Block-level tags ────────────────────────────────
            Event::Start(Tag::CodeBlock(_)) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
                in_code_block = true;
                code_block_text.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let block_text = std::mem::take(&mut code_block_text);
                let trimmed = block_text.trim_end_matches('\n');
                let bg = code_block_bg(is_mine);
                let el: Element<'a, Message, Theme> = container(
                    text(trimmed.to_string())
                        .size(13)
                        .font(MONO_FONT)
                        .color(text_color),
                )
                .padding([8, 10])
                .width(iced::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(Background::Color(bg)),
                    border: border::rounded(6),
                    ..Default::default()
                })
                .into();
                target(&mut blocks, &mut bq_stack).push(el);
            }
            Event::Start(Tag::BlockQuote) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
                bq_stack.push(Vec::new());
            }
            Event::End(TagEnd::BlockQuote) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
                let inner = bq_stack.pop().unwrap_or_default();
                if !inner.is_empty() {
                    let el = wrap_blockquote(inner, is_mine);
                    target(&mut blocks, &mut bq_stack).push(el);
                }
            }
            Event::Start(Tag::List(ordered)) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
                list_index = ordered;
            }
            Event::End(TagEnd::List(_)) => {
                list_index = None;
            }
            Event::Start(Tag::Item) => {}
            Event::End(TagEnd::Item) => {
                let bullet = match list_index {
                    Some(ref mut n) => {
                        let s = format!("{}.", n);
                        *n += 1;
                        s
                    }
                    None => "•".to_string(),
                };
                if !inline_spans.is_empty() {
                    let rt: Element<'a, Message, Theme> =
                        rich_text(std::mem::take(&mut inline_spans)).size(15).into();
                    let item: Element<'a, Message, Theme> =
                        row![text(bullet).size(15).color(text_color), rt,]
                            .spacing(6)
                            .into();
                    target(&mut blocks, &mut bq_stack).push(item);
                }
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
            }
            Event::Start(Tag::Heading { .. }) => {
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
                style.bold = true;
            }
            Event::End(TagEnd::Heading(_)) => {
                style.bold = false;
                flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));
            }

            // ── Inline tags ────────────────────────────────────
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strikethrough) => style.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => style.strikethrough = false,
            Event::Start(Tag::Link { .. }) => style.link = true,
            Event::End(TagEnd::Link) => style.link = false,

            Event::Code(code) => {
                inline_spans.push(
                    span(Cow::Owned(code.to_string()))
                        .font(MONO_FONT)
                        .size(13)
                        .color(text_color),
                );
            }

            Event::Text(txt) => {
                if in_code_block {
                    code_block_text.push_str(&txt);
                    continue;
                }
                let mut s = span(Cow::Owned(txt.to_string())).color(text_color);
                if let Some(f) = style.font() {
                    s = s.font(f);
                }
                if style.strikethrough {
                    s = s.strikethrough(true);
                }
                if style.link {
                    s = s.color(theme::accent_blue()).underline(true);
                }
                inline_spans.push(s);
            }

            Event::SoftBreak => {
                inline_spans.push(span(Cow::Borrowed(" ")));
            }
            Event::HardBreak => {
                inline_spans.push(span(Cow::Borrowed("\n")));
            }

            _ => {}
        }
    }

    // Flush any remaining inline spans.
    flush_spans(&mut inline_spans, target(&mut blocks, &mut bq_stack));

    // Close any unclosed blockquotes (malformed input).
    while let Some(inner) = bq_stack.pop() {
        if !inner.is_empty() {
            let el = wrap_blockquote(inner, is_mine);
            target(&mut blocks, &mut bq_stack).push(el);
        }
    }

    match blocks.len() {
        0 => Space::new().width(0).height(0).into(),
        1 => blocks.pop().unwrap(),
        _ => column(blocks).spacing(4).into(),
    }
}

/// Returns `true` if the content has any markdown syntax worth rendering.
/// Plain text with no formatting is faster to render as a simple `text()`.
pub fn has_markdown(content: &str) -> bool {
    content.bytes().any(|b| {
        matches!(
            b,
            b'*' | b'_' | b'`' | b'~' | b'[' | b'#' | b'>' | b'-' | b'+' | b'0'..=b'9'
        )
    })
}
