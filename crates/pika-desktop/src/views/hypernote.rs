use std::borrow::Cow;
use std::collections::HashMap;

use iced::advanced::text::Span;
use iced::widget::{button, column, container, rich_text, row, rule, span, text, Space};
use iced::{border, font, Alignment, Background, Border, Color, Element, Font, Length, Theme};
use pika_core::{HypernoteData, HypernoteDocument, HypernoteNode, HypernoteNodeType};

use super::avatar::{avatar_circle, AvatarCache};
use super::conversation::Message;
use crate::icons;
use crate::theme;

const MONO_FONT: Font = Font::with_name("monospace");

const BOLD_FONT: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

const ITALIC_FONT: Font = Font {
    style: font::Style::Italic,
    ..Font::DEFAULT
};

const BOLD_ITALIC_FONT: Font = Font {
    weight: font::Weight::Bold,
    style: font::Style::Italic,
    ..Font::DEFAULT
};

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

pub fn render_hypernote<'a>(
    message_id: &'a str,
    hypernote: &'a HypernoteData,
    optimistic_selected_action: Option<&'a str>,
    avatar_cache: &mut AvatarCache,
) -> Element<'a, Message, Theme> {
    let document = &hypernote.document;
    let selected_action = hypernote
        .my_response
        .as_deref()
        .or(optimistic_selected_action);
    if document.root_node_ids.is_empty() {
        return text("Unsupported hypernote")
            .size(14)
            .color(theme::text_secondary())
            .into();
    }

    let mut content = column!().spacing(8);
    for &node_id in &document.root_node_ids {
        content = content.push(render_node(
            document,
            hypernote,
            message_id,
            selected_action,
            node_id,
        ));
    }

    if !hypernote.responders.is_empty() {
        content = content.push(render_responders(hypernote, avatar_cache));
    }

    content.into()
}

fn render_node<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node_id: u32,
) -> Element<'a, Message, Theme> {
    let Some(node) = document.nodes.get(node_id as usize) else {
        return Space::new().width(0).height(0).into();
    };

    match node.node_type {
        HypernoteNodeType::Heading => render_heading(document, node),
        HypernoteNodeType::Paragraph => {
            render_paragraph(document, hypernote, message_id, selected_action, node)
        }
        HypernoteNodeType::Strong
        | HypernoteNodeType::Emphasis
        | HypernoteNodeType::CodeInline
        | HypernoteNodeType::Link
        | HypernoteNodeType::Text => render_inline_rich_text(
            document,
            &[node.id],
            15.0,
            theme::text_primary(),
            InlineStyle::default(),
        )
        .unwrap_or_else(empty_element),
        HypernoteNodeType::CodeBlock => render_code_block(node),
        HypernoteNodeType::Image => render_image(document, node),
        HypernoteNodeType::ListUnordered | HypernoteNodeType::ListOrdered => {
            render_list(document, hypernote, message_id, selected_action, node)
        }
        HypernoteNodeType::ListItem => {
            render_list_item(document, hypernote, message_id, selected_action, node, None)
        }
        HypernoteNodeType::Blockquote => {
            render_blockquote(document, hypernote, message_id, selected_action, node)
        }
        HypernoteNodeType::Hr => container(rule::horizontal(1).style(theme::subtle_rule_style))
            .padding([4, 0])
            .into(),
        HypernoteNodeType::HardBreak => Space::new().height(4).width(0).into(),
        HypernoteNodeType::MdxJsxElement | HypernoteNodeType::MdxJsxSelfClosing => {
            render_jsx(document, hypernote, message_id, selected_action, node)
        }
        HypernoteNodeType::Unsupported => {
            render_unsupported(document, hypernote, message_id, selected_action, node)
        }
    }
}

fn render_heading<'a>(
    document: &'a HypernoteDocument,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let size = match node.level.unwrap_or(1) {
        1 => 22.0,
        2 => 19.0,
        3 => 17.0,
        _ => 16.0,
    };

    render_inline_rich_text(
        document,
        &node.child_ids,
        size,
        theme::text_primary(),
        InlineStyle {
            bold: true,
            ..InlineStyle::default()
        },
    )
    .unwrap_or_else(empty_element)
}

fn render_paragraph<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    if has_only_inline_children(document, &node.child_ids) {
        return render_inline_rich_text(
            document,
            &node.child_ids,
            15.0,
            theme::text_primary(),
            InlineStyle::default(),
        )
        .unwrap_or_else(empty_element);
    }

    render_children_column(
        document,
        hypernote,
        message_id,
        selected_action,
        &node.child_ids,
        4.0,
    )
}

fn render_code_block<'a>(node: &'a HypernoteNode) -> Element<'a, Message, Theme> {
    let mut body = column!().spacing(6);
    if let Some(lang) = node.lang.as_deref().filter(|lang| !lang.is_empty()) {
        body = body.push(text(lang).size(11).color(theme::text_secondary()));
    }

    body = body.push(
        text(node.value.as_deref().unwrap_or_default())
            .size(13)
            .font(MONO_FONT)
            .color(theme::text_primary()),
    );

    container(body)
        .padding([8, 10])
        .width(Length::Fill)
        .style(|_theme: &Theme| container::Style {
            background: Some(Background::Color(theme::hover_bg())),
            border: border::rounded(8),
            ..Default::default()
        })
        .into()
}

fn render_image<'a>(
    document: &'a HypernoteDocument,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let label = extract_text(document, &node.child_ids);
    let url = node.url.as_deref().unwrap_or("");
    let caption = if label.trim().is_empty() {
        format!("Image: {url}")
    } else {
        format!("{label} ({url})")
    };

    container(text(caption).size(13).color(theme::accent_blue()))
        .padding([8, 10])
        .style(|_theme: &Theme| container::Style {
            border: Border {
                color: theme::input_border(),
                width: 1.0,
                radius: border::radius(8),
            },
            ..Default::default()
        })
        .into()
}

fn render_list<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let ordered = matches!(node.node_type, HypernoteNodeType::ListOrdered);
    let mut items = column!().spacing(4);

    for (index, &child_id) in node.child_ids.iter().enumerate() {
        let bullet = if ordered {
            format!("{}.", index + 1)
        } else {
            "•".to_string()
        };
        let child = document.nodes.get(child_id as usize);
        items = items.push(match child {
            Some(child) => render_list_item(
                document,
                hypernote,
                message_id,
                selected_action,
                child,
                Some(bullet),
            ),
            None => empty_element(),
        });
    }

    items.into()
}

fn render_list_item<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
    bullet_override: Option<String>,
) -> Element<'a, Message, Theme> {
    let marker = if let Some(bullet) = bullet_override {
        text(bullet).size(15).color(theme::text_secondary()).into()
    } else if let Some(checked) = node.checked {
        checklist_indicator(checked, 16.0)
    } else {
        text("•").size(15).color(theme::text_secondary()).into()
    };

    let content = if has_only_inline_children(document, &node.child_ids) {
        render_inline_rich_text(
            document,
            &node.child_ids,
            15.0,
            theme::text_primary(),
            InlineStyle::default(),
        )
        .unwrap_or_else(empty_element)
    } else {
        render_children_column(
            document,
            hypernote,
            message_id,
            selected_action,
            &node.child_ids,
            2.0,
        )
    };

    row![marker, content].spacing(8).into()
}

fn render_blockquote<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    row![
        container(Space::new().width(Length::Fixed(3.0)).height(Length::Fill)).style(
            |_: &Theme| container::Style {
                background: Some(Background::Color(theme::text_secondary().scale_alpha(0.55))),
                border: border::rounded(2),
                ..Default::default()
            }
        ),
        render_children_column(
            document,
            hypernote,
            message_id,
            selected_action,
            &node.child_ids,
            4.0,
        ),
    ]
    .spacing(10)
    .into()
}

fn render_jsx<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    match node.name.as_deref().unwrap_or_default() {
        "Card" => container(render_children_column(
            document,
            hypernote,
            message_id,
            selected_action,
            &node.child_ids,
            8.0,
        ))
        .padding(12)
        .style(|_theme: &Theme| container::Style {
            background: Some(Background::Color(theme::hover_bg())),
            border: border::rounded(12),
            ..Default::default()
        })
        .into(),
        "VStack" => {
            let gap = attribute_i32(node, &["spacing", "gap"]).unwrap_or(8) as f32;
            render_children_column(
                document,
                hypernote,
                message_id,
                selected_action,
                &node.child_ids,
                gap,
            )
        }
        "HStack" => {
            let gap = attribute_i32(node, &["spacing", "gap"]).unwrap_or(8) as f32;
            let mut children = row!().spacing(gap);
            for &child_id in &node.child_ids {
                children = children.push(render_node(
                    document,
                    hypernote,
                    message_id,
                    selected_action,
                    child_id,
                ));
            }
            children.into()
        }
        "Heading" => render_inline_rich_text(
            document,
            &node.child_ids,
            16.0,
            theme::text_primary(),
            InlineStyle {
                bold: true,
                ..InlineStyle::default()
            },
        )
        .unwrap_or_else(empty_element),
        "Body" => render_inline_rich_text(
            document,
            &node.child_ids,
            15.0,
            theme::text_primary(),
            InlineStyle::default(),
        )
        .unwrap_or_else(empty_element),
        "Caption" => render_inline_rich_text(
            document,
            &node.child_ids,
            13.0,
            theme::text_secondary(),
            InlineStyle::default(),
        )
        .unwrap_or_else(empty_element),
        "TextInput" => render_text_input(node, hypernote),
        "SubmitButton" => {
            render_submit_button(document, hypernote, message_id, selected_action, node)
        }
        "ChecklistItem" => render_checklist_item(document, hypernote, node),
        "Details" => render_details(document, hypernote, message_id, selected_action, node),
        "Summary" => render_inline_rich_text(
            document,
            &node.child_ids,
            14.0,
            theme::text_primary(),
            InlineStyle {
                bold: true,
                ..InlineStyle::default()
            },
        )
        .unwrap_or_else(empty_element),
        _ => container(render_children_column(
            document,
            hypernote,
            message_id,
            selected_action,
            &node.child_ids,
            4.0,
        ))
        .padding(8)
        .style(|_theme: &Theme| container::Style {
            border: Border {
                color: theme::input_border(),
                width: 1.0,
                radius: border::radius(8),
            },
            ..Default::default()
        })
        .into(),
    }
}

fn render_text_input<'a>(
    node: &'a HypernoteNode,
    hypernote: &'a HypernoteData,
) -> Element<'a, Message, Theme> {
    let field_name = attribute_value(node, "name").unwrap_or("field");
    let placeholder = attribute_value(node, "placeholder").unwrap_or("");
    let value = form_value(hypernote, field_name).unwrap_or("");
    let display = if value.is_empty() { placeholder } else { value };
    let text_color = if value.is_empty() {
        theme::text_faded()
    } else {
        theme::text_primary()
    };

    container(text(display).size(14).color(text_color))
        .padding([10, 12])
        .width(Length::Fill)
        .style(|_theme: &Theme| container::Style {
            background: Some(Background::Color(
                theme::current().background.component.disabled,
            )),
            border: Border {
                color: theme::input_border(),
                width: 1.0,
                radius: border::radius(8),
            },
            ..Default::default()
        })
        .into()
}

fn render_submit_button<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let action = attribute_value(node, "action").unwrap_or("submit");
    let variant = attribute_value(node, "variant").unwrap_or("primary");
    let tally = hypernote
        .response_tallies
        .iter()
        .find(|tally| tally.action == action);
    let is_selected = selected_action == Some(action);
    let is_unselected = selected_action.is_some() && !is_selected;
    let label = extract_text(document, &node.child_ids);
    let label = if label.trim().is_empty() {
        Cow::Borrowed(action)
    } else {
        Cow::Owned(label)
    };

    let mut row_content = row!().spacing(6).align_y(iced::Alignment::Center);
    if is_selected {
        row_content = row_content.push(
            text(icons::CHECK)
                .font(icons::LUCIDE_FONT)
                .size(14)
                .color(button_foreground(variant, is_selected)),
        );
    }
    row_content = row_content.push(
        text(label)
            .size(14)
            .font(icons::MEDIUM)
            .color(button_foreground(variant, is_selected)),
    );
    if let Some(tally) = tally {
        row_content = row_content.push(
            text(tally.count.to_string())
                .size(12)
                .color(button_foreground(variant, is_selected).scale_alpha(0.9)),
        );
    }

    let message = Message::HypernoteAction {
        message_id: message_id.to_string(),
        action_name: action.to_string(),
        form: default_form_map(hypernote),
    };

    button(container(row_content).padding([9, 12]).width(Length::Fill))
        .padding(0)
        .width(Length::Fill)
        .on_press_maybe((selected_action.is_none()).then_some(message))
        .style(move |_theme: &Theme, status| {
            submit_button_style(variant, is_selected, is_unselected, status)
        })
        .into()
}

fn render_checklist_item<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let field_name = attribute_value(node, "name").unwrap_or("item");
    let is_checked = form_value(hypernote, field_name)
        .map(|value| value == "true")
        .unwrap_or_else(|| has_boolean_attribute(node, "checked"));
    let text_color = if is_checked {
        theme::text_secondary()
    } else {
        theme::text_primary()
    };

    let label = render_inline_rich_text(
        document,
        &node.child_ids,
        15.0,
        text_color,
        InlineStyle {
            strikethrough: is_checked,
            ..InlineStyle::default()
        },
    )
    .unwrap_or_else(empty_element);

    row![checklist_indicator(is_checked, 18.0), label]
        .spacing(8)
        .align_y(iced::Alignment::Start)
        .into()
}

fn render_details<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    let mut summary_text = "Details".to_string();
    let mut body_ids = Vec::new();

    for &child_id in &node.child_ids {
        let Some(child) = document.nodes.get(child_id as usize) else {
            continue;
        };
        let is_summary = matches!(
            child.node_type,
            HypernoteNodeType::MdxJsxElement | HypernoteNodeType::MdxJsxSelfClosing
        ) && child.name.as_deref() == Some("Summary");

        if is_summary {
            let summary = extract_text(document, &child.child_ids);
            if !summary.trim().is_empty() {
                summary_text = summary;
            }
        } else {
            body_ids.push(child_id);
        }
    }

    column![
        row![
            text("Details").size(12).color(theme::text_secondary()),
            text(summary_text)
                .size(14)
                .font(icons::MEDIUM)
                .color(theme::text_primary()),
        ]
        .spacing(8),
        row![
            Space::new().width(Length::Fixed(20.0)),
            render_children_column(
                document,
                hypernote,
                message_id,
                selected_action,
                &body_ids,
                8.0,
            ),
        ]
        .spacing(0),
    ]
    .spacing(6)
    .into()
}

fn render_unsupported<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    node: &'a HypernoteNode,
) -> Element<'a, Message, Theme> {
    if !node.child_ids.is_empty() {
        return render_children_column(
            document,
            hypernote,
            message_id,
            selected_action,
            &node.child_ids,
            4.0,
        );
    }

    if let Some(value) = node.value.as_deref() {
        return text(value).size(14).color(theme::text_secondary()).into();
    }

    empty_element()
}

fn render_children_column<'a>(
    document: &'a HypernoteDocument,
    hypernote: &'a HypernoteData,
    message_id: &'a str,
    selected_action: Option<&'a str>,
    child_ids: &[u32],
    spacing: f32,
) -> Element<'a, Message, Theme> {
    let mut children = column!().spacing(spacing);
    for &child_id in child_ids {
        children = children.push(render_node(
            document,
            hypernote,
            message_id,
            selected_action,
            child_id,
        ));
    }
    children.into()
}

fn render_responders<'a>(
    hypernote: &'a HypernoteData,
    avatar_cache: &mut AvatarCache,
) -> Element<'a, Message, Theme> {
    let mut responders = row!().spacing(-6.0).align_y(Alignment::Center);
    for responder in hypernote.responders.iter().take(5) {
        responders = responders.push(avatar_circle(
            responder.name.as_deref(),
            responder.picture_url.as_deref(),
            20.0,
            avatar_cache,
        ));
    }
    if hypernote.responders.len() > 5 {
        responders = responders.push(
            text(format!("+{}", hypernote.responders.len() - 5))
                .size(11)
                .font(icons::MEDIUM)
                .color(theme::text_secondary()),
        );
    }
    column![Space::new().height(4), responders]
        .spacing(0)
        .into()
}

fn render_inline_rich_text<'a>(
    document: &'a HypernoteDocument,
    node_ids: &[u32],
    size: f32,
    text_color: Color,
    style: InlineStyle,
) -> Option<Element<'a, Message, Theme>> {
    let mut spans: Vec<Span<'a, (), Font>> = Vec::new();
    collect_inline_spans(document, node_ids, &mut spans, style, text_color);
    if spans.is_empty() {
        None
    } else {
        Some(rich_text(spans).size(size).into())
    }
}

fn collect_inline_spans<'a>(
    document: &'a HypernoteDocument,
    node_ids: &[u32],
    spans: &mut Vec<Span<'a, (), Font>>,
    style: InlineStyle,
    text_color: Color,
) {
    for &node_id in node_ids {
        let Some(node) = document.nodes.get(node_id as usize) else {
            continue;
        };

        match node.node_type {
            HypernoteNodeType::Text => {
                if let Some(value) = node.value.as_deref().filter(|value| !value.is_empty()) {
                    spans.push(styled_span(
                        Cow::Owned(value.to_string()),
                        &style,
                        text_color,
                    ));
                }
            }
            HypernoteNodeType::Strong => {
                let mut nested = style.clone();
                nested.bold = true;
                collect_inline_spans(document, &node.child_ids, spans, nested, text_color);
            }
            HypernoteNodeType::Emphasis => {
                let mut nested = style.clone();
                nested.italic = true;
                collect_inline_spans(document, &node.child_ids, spans, nested, text_color);
            }
            HypernoteNodeType::CodeInline => {
                if let Some(value) = node.value.as_deref() {
                    let mut code_span = span(Cow::Owned(value.to_string()))
                        .font(MONO_FONT)
                        .size(13)
                        .color(text_color);
                    if style.strikethrough {
                        code_span = code_span.strikethrough(true);
                    }
                    spans.push(code_span);
                }
            }
            HypernoteNodeType::Link => {
                let mut nested = style.clone();
                nested.link = true;
                if node.child_ids.is_empty() {
                    let label = node.url.as_deref().unwrap_or_default().to_string();
                    if !label.is_empty() {
                        spans.push(styled_span(Cow::Owned(label), &nested, text_color));
                    }
                } else {
                    collect_inline_spans(document, &node.child_ids, spans, nested, text_color);
                }
            }
            HypernoteNodeType::HardBreak => spans.push(span(Cow::Borrowed("\n"))),
            HypernoteNodeType::Unsupported => {
                if !node.child_ids.is_empty() {
                    collect_inline_spans(
                        document,
                        &node.child_ids,
                        spans,
                        style.clone(),
                        text_color,
                    );
                } else if let Some(value) = node.value.as_deref().filter(|value| !value.is_empty())
                {
                    spans.push(styled_span(
                        Cow::Owned(value.to_string()),
                        &style,
                        text_color,
                    ));
                }
            }
            _ => {
                if !node.child_ids.is_empty() {
                    collect_inline_spans(
                        document,
                        &node.child_ids,
                        spans,
                        style.clone(),
                        text_color,
                    );
                }
            }
        }
    }
}

fn styled_span<'a>(
    value: Cow<'a, str>,
    style: &InlineStyle,
    text_color: Color,
) -> Span<'a, (), Font> {
    let mut styled = span(value).color(if style.link {
        theme::accent_blue()
    } else {
        text_color
    });
    if let Some(font) = style.font() {
        styled = styled.font(font);
    }
    if style.strikethrough {
        styled = styled.strikethrough(true);
    }
    if style.link {
        styled = styled.underline(true);
    }
    styled
}

fn has_only_inline_children(document: &HypernoteDocument, child_ids: &[u32]) -> bool {
    child_ids.iter().all(|child_id| {
        document
            .nodes
            .get(*child_id as usize)
            .map(|node| is_inline_node(document, node))
            .unwrap_or(false)
    })
}

fn is_inline_node(document: &HypernoteDocument, node: &HypernoteNode) -> bool {
    match node.node_type {
        HypernoteNodeType::Text
        | HypernoteNodeType::Strong
        | HypernoteNodeType::Emphasis
        | HypernoteNodeType::CodeInline
        | HypernoteNodeType::Link
        | HypernoteNodeType::HardBreak => true,
        HypernoteNodeType::Unsupported => {
            !node.child_ids.is_empty() && has_only_inline_children(document, &node.child_ids)
                || node.value.is_some()
        }
        _ => false,
    }
}

fn extract_text(document: &HypernoteDocument, node_ids: &[u32]) -> String {
    let mut out = String::new();
    for &node_id in node_ids {
        let Some(node) = document.nodes.get(node_id as usize) else {
            continue;
        };
        match node.node_type {
            HypernoteNodeType::Text | HypernoteNodeType::CodeInline => {
                out.push_str(node.value.as_deref().unwrap_or_default());
            }
            HypernoteNodeType::Link => {
                if node.child_ids.is_empty() {
                    out.push_str(node.url.as_deref().unwrap_or_default());
                } else {
                    out.push_str(&extract_text(document, &node.child_ids));
                }
            }
            HypernoteNodeType::HardBreak => out.push('\n'),
            _ => {
                if !node.child_ids.is_empty() {
                    out.push_str(&extract_text(document, &node.child_ids));
                } else if let Some(value) = node.value.as_deref() {
                    out.push_str(value);
                }
            }
        }
    }
    out
}

fn attribute_value<'a>(node: &'a HypernoteNode, name: &str) -> Option<&'a str> {
    node.attributes
        .iter()
        .find(|attr| attr.name == name)
        .and_then(|attr| attr.value.as_deref())
}

fn attribute_i32(node: &HypernoteNode, names: &[&str]) -> Option<i32> {
    names
        .iter()
        .find_map(|name| attribute_value(node, name))
        .and_then(|value| value.parse::<i32>().ok())
}

fn has_boolean_attribute(node: &HypernoteNode, name: &str) -> bool {
    attribute_value(node, name)
        .map(|value| value == "true")
        .unwrap_or(false)
}

fn form_value<'a>(hypernote: &'a HypernoteData, name: &str) -> Option<&'a str> {
    hypernote
        .default_form_state
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.as_str())
}

fn button_foreground(variant: &str, is_selected: bool) -> Color {
    if is_selected || variant == "primary" || variant == "danger" {
        Color::WHITE
    } else {
        theme::text_primary()
    }
}

fn submit_button_style(
    variant: &str,
    is_selected: bool,
    is_unselected: bool,
    status: button::Status,
) -> button::Style {
    let background = if is_selected || variant == "primary" {
        if variant == "danger" {
            theme::danger()
        } else {
            theme::accent_blue()
        }
    } else {
        theme::hover_bg()
    };

    let background = match status {
        button::Status::Hovered if is_selected || variant == "primary" || variant == "danger" => {
            background.scale_alpha(0.92)
        }
        button::Status::Hovered => background.scale_alpha(0.82),
        _ => background,
    };

    button::Style {
        text_color: button_foreground(variant, is_selected),
        background: Some(Background::Color(
            background.scale_alpha(if is_unselected { 0.55 } else { 1.0 }),
        )),
        border: Border {
            color: if is_selected || variant == "primary" {
                background
            } else {
                theme::input_border()
            },
            width: 1.0,
            radius: border::radius(9),
        },
        ..Default::default()
    }
}

fn default_form_map(hypernote: &HypernoteData) -> HashMap<String, String> {
    hypernote
        .default_form_state
        .iter()
        .map(|field| (field.name.clone(), field.value.clone()))
        .collect()
}

fn checklist_indicator<'a>(is_checked: bool, size: f32) -> Element<'a, Message, Theme> {
    let icon = if is_checked { icons::CHECK } else { "" };
    container(
        text(icon)
            .font(icons::LUCIDE_FONT)
            .size(size * 0.7)
            .color(if is_checked {
                Color::WHITE
            } else {
                Color::TRANSPARENT
            }),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_theme: &Theme| theme::current().checkbox_indicator(is_checked))
    .into()
}

fn empty_element<'a>() -> Element<'a, Message, Theme> {
    Space::new().width(0).height(0).into()
}
