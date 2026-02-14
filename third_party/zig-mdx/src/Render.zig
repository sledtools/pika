const std = @import("std");
const Ast = @import("Ast.zig");
const Token = @import("Token.zig");
const Allocator = std.mem.Allocator;

/// Render an AST back to canonical MDX source.
/// This enables roundtripping: source -> AST -> source
pub fn render(ast: *const Ast, output: *std.ArrayList(u8), allocator: Allocator) !void {
    const writer = output.writer(allocator);

    // Find the document node
    var doc_idx: ?Ast.NodeIndex = null;
    for (0..ast.nodes.len) |i| {
        const idx: Ast.NodeIndex = @intCast(i);
        if (ast.nodes.get(idx).tag == .document) {
            doc_idx = idx;
            break;
        }
    }

    if (doc_idx) |idx| {
        const children = ast.children(idx);
        var last_was_content = false;
        for (children) |child_idx| {
            const child_node = ast.nodes.get(child_idx);

            // Skip empty paragraphs
            if (child_node.tag == .paragraph) {
                const para_children = ast.children(child_idx);
                if (para_children.len == 0) continue;
                if (para_children.len == 1) {
                    const para_child = ast.nodes.get(para_children[0]);
                    if (para_child.tag == .text) {
                        const text = ast.tokenSlice(para_child.main_token);
                        const trimmed = std.mem.trim(u8, text, " \t\n\r");
                        if (trimmed.len == 0) continue;
                    }
                }
            }

            // Add blank line between content blocks
            if (last_was_content) {
                try writer.writeByte('\n');
            }

            try renderNode(ast, child_idx, writer, .{});
            last_was_content = (child_node.tag != .frontmatter);
        }
    }
}

const RenderContext = struct {
    in_list: bool = false,
    list_index: u32 = 0,
    indent_level: u32 = 0,
    in_jsx: bool = false,
};

fn writeIndent(writer: anytype, level: u32) !void {
    for (0..level) |_| {
        try writer.writeAll("  ");
    }
}

/// Check if a JSX element can be rendered on a single line
/// Returns true if it has a single simple child (text or expression)
fn canRenderJsxInline(ast: *const Ast, children: []const Ast.NodeIndex) bool {
    if (children.len != 1) return false;
    const child = ast.nodes.get(children[0]);
    return child.tag == .text or child.tag == .mdx_text_expression;
}

/// Check if a node is a "content block" that should have blank lines between siblings
/// Only expressions and content-like nodes, not JSX structure
fn isContentBlock(tag: Ast.Node.Tag) bool {
    return switch (tag) {
        .mdx_text_expression,
        .mdx_flow_expression,
        .paragraph,
        .heading,
        .code_block,
        .blockquote,
        .list_unordered,
        .list_ordered,
        => true,
        else => false,
    };
}

fn renderNode(ast: *const Ast, node_idx: Ast.NodeIndex, writer: anytype, ctx: RenderContext) !void {
    const node = ast.nodes.get(node_idx);

    switch (node.tag) {
        .document => {
            // Shouldn't be called directly, but handle it
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, ctx);
            }
        },

        .frontmatter => {
            try writer.writeAll("---\n");
            const range = ast.extraData(node.data.extra, Ast.Node.Range);
            const content = extractTokenRangeContent(ast, range);
            try writer.writeAll(content);
            if (content.len > 0 and content[content.len - 1] != '\n') {
                try writer.writeByte('\n');
            }
            try writer.writeAll("---\n\n");
        },

        .heading => {
            const info = ast.headingInfo(node_idx);
            // Write heading markers
            for (0..info.level) |_| {
                try writer.writeByte('#');
            }
            try writer.writeByte(' ');
            // Render children (inline content)
            const children = @as(
                []const Ast.NodeIndex,
                @ptrCast(ast.extra_data[info.children_start..info.children_end]),
            );
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, ctx);
            }
            try writer.writeByte('\n');
        },

        .paragraph => {
            const children = ast.children(node_idx);
            // Skip empty paragraphs (just whitespace/blank lines)
            if (children.len == 0) return;
            // Check if paragraph only has empty text
            if (children.len == 1) {
                const child = ast.nodes.get(children[0]);
                if (child.tag == .text) {
                    const text = ast.tokenSlice(child.main_token);
                    const trimmed = std.mem.trim(u8, text, " \t\n\r");
                    if (trimmed.len == 0) return;
                }
            }
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, ctx);
            }
            if (!ctx.in_jsx) {
                try writer.writeByte('\n');
            }
        },

        .text => {
            const text = ast.tokenSlice(node.main_token);
            try writer.writeAll(text);
        },

        .strong => {
            try writer.writeAll("**");
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, ctx);
            }
            try writer.writeAll("**");
        },

        .emphasis => {
            try writer.writeByte('*');
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, ctx);
            }
            try writer.writeByte('*');
        },

        .code_inline => {
            try writer.writeByte('`');
            const content_token = node.data.token;
            const text = ast.tokenSlice(content_token);
            try writer.writeAll(text);
            try writer.writeByte('`');
        },

        .code_block => {
            try writer.writeAll("```");
            // Extract language if present
            const fence_token = node.main_token;
            const token_tags = ast.tokens.items(.tag);

            if (fence_token + 1 < ast.tokens.len) {
                const next_token = fence_token + 1;
                if (token_tags[next_token] == .text) {
                    const lang_text = ast.tokenSlice(next_token);
                    const trimmed = std.mem.trim(u8, lang_text, " \t\n\r");
                    if (trimmed.len > 0) {
                        try writer.writeAll(trimmed);
                    }
                }
            }
            try writer.writeByte('\n');

            // Extract code content
            const code = extractCodeBlockContent(ast, fence_token);
            try writer.writeAll(code);
            if (code.len > 0 and code[code.len - 1] != '\n') {
                try writer.writeByte('\n');
            }
            try writer.writeAll("```\n");
        },

        .blockquote => {
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try writer.writeAll("> ");
                try renderNode(ast, child_idx, writer, ctx);
            }
        },

        .list_unordered => {
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, .{
                    .in_list = true,
                    .list_index = 0, // unordered
                    .indent_level = ctx.indent_level,
                    .in_jsx = ctx.in_jsx,
                });
            }
        },

        .list_ordered => {
            const children = ast.children(node_idx);
            for (children, 1..) |child_idx, i| {
                try renderNode(ast, child_idx, writer, .{
                    .in_list = true,
                    .list_index = @intCast(i),
                    .indent_level = ctx.indent_level,
                    .in_jsx = ctx.in_jsx,
                });
            }
        },

        .list_item => {
            // Write indent
            try writeIndent(writer, ctx.indent_level);
            // Write marker
            if (ctx.list_index == 0) {
                try writer.writeAll("- ");
            } else {
                try writer.print("{d}. ", .{ctx.list_index});
            }
            // Render children inline
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                const child = ast.nodes.get(child_idx);
                // For paragraph inside list item, render inline content only
                if (child.tag == .paragraph) {
                    const para_children = ast.children(child_idx);
                    for (para_children) |para_child_idx| {
                        try renderNode(ast, para_child_idx, writer, ctx);
                    }
                } else {
                    try renderNode(ast, child_idx, writer, ctx);
                }
            }
            try writer.writeByte('\n');
        },

        .hr => {
            try writer.writeAll("---\n");
        },

        .hard_break => {
            try writer.writeAll("  \n");
        },

        .link => {
            const link = ast.extraData(node.data.extra, Ast.Link);
            try writer.writeByte('[');
            // Render link text
            if (link.text_node.unwrap()) |text_idx| {
                try renderNode(ast, text_idx, writer, ctx);
            }
            try writer.writeAll("](");
            const url = ast.tokenSlice(link.url_token);
            try writer.writeAll(url);
            try writer.writeByte(')');
        },

        .image => {
            const link = ast.extraData(node.data.extra, Ast.Link);
            try writer.writeAll("![");
            // Render alt text
            if (link.text_node.unwrap()) |text_idx| {
                try renderNode(ast, text_idx, writer, ctx);
            }
            try writer.writeAll("](");
            const url = ast.tokenSlice(link.url_token);
            try writer.writeAll(url);
            try writer.writeByte(')');
        },

        .mdx_text_expression => {
            try writer.writeByte('{');
            const range = ast.extraData(node.data.extra, Ast.Node.Range);
            const content = extractTokenRangeContent(ast, range);
            const trimmed = std.mem.trim(u8, content, " \t\n\r");
            try writer.writeAll(trimmed);
            try writer.writeByte('}');
        },

        .mdx_flow_expression => {
            // Block-level expression
            try writer.writeByte('{');
            const range = ast.extraData(node.data.extra, Ast.Node.Range);
            const content = extractTokenRangeContent(ast, range);
            const trimmed = std.mem.trim(u8, content, " \t\n\r");
            try writer.writeAll(trimmed);
            try writer.writeByte('}');
            try writer.writeByte('\n');
        },

        .mdx_jsx_element => {
            const elem = ast.jsxElement(node_idx);
            const name_raw = ast.tokenSlice(elem.name_token);
            const name = std.mem.trim(u8, name_raw, " \t\n\r");

            const children = @as(
                []const Ast.NodeIndex,
                @ptrCast(ast.extra_data[elem.children_start..elem.children_end]),
            );

            // Check if we can render inline (single simple child)
            const render_inline = canRenderJsxInline(ast, children);

            // Write opening tag with indent
            try writeIndent(writer, ctx.indent_level);
            try writer.writeByte('<');
            try writer.writeAll(name);
            try renderJsxAttributes(ast, node_idx, writer);
            try writer.writeByte('>');

            if (render_inline) {
                // Render single child inline (no newline, no indent)
                try renderNode(ast, children[0], writer, .{
                    .indent_level = ctx.indent_level + 1,
                    .in_jsx = true,
                });
            } else {
                // Render children on separate lines with increased indent
                try writer.writeByte('\n');
                var prev_was_content_block = false;
                for (children, 0..) |child_idx, i| {
                    const child = ast.nodes.get(child_idx);
                    const is_content = isContentBlock(child.tag);

                    // Add blank line between content blocks (expressions, paragraphs, etc.)
                    if (prev_was_content_block and is_content) {
                        try writer.writeByte('\n');
                    }

                    try renderNode(ast, child_idx, writer, .{
                        .indent_level = ctx.indent_level + 1,
                        .in_jsx = true,
                    });

                    // Handle newlines: hard_break includes its own newline,
                    // and we don't want a newline before hard_break
                    const next_is_hard_break = if (i + 1 < children.len)
                        ast.nodes.get(children[i + 1]).tag == .hard_break
                    else
                        false;

                    if (child.tag != .hard_break and !next_is_hard_break) {
                        try writer.writeByte('\n');
                    }
                    prev_was_content_block = is_content;
                }
                try writeIndent(writer, ctx.indent_level);
            }

            try writer.writeAll("</");
            try writer.writeAll(name);
            try writer.writeByte('>');

            // Only add newline if we're at top level (not nested in JSX)
            if (!ctx.in_jsx) {
                try writer.writeByte('\n');
            }
        },

        .mdx_jsx_self_closing => {
            const elem = ast.jsxElement(node_idx);
            const name_raw = ast.tokenSlice(elem.name_token);
            const name = std.mem.trim(u8, name_raw, " \t\n\r");

            try writeIndent(writer, ctx.indent_level);
            try writer.writeByte('<');
            try writer.writeAll(name);
            try renderJsxAttributes(ast, node_idx, writer);
            try writer.writeAll(" />");

            if (!ctx.in_jsx) {
                try writer.writeByte('\n');
            }
        },

        .mdx_jsx_fragment => {
            try writeIndent(writer, ctx.indent_level);
            try writer.writeAll("<>\n");
            const children = ast.children(node_idx);
            for (children) |child_idx| {
                try renderNode(ast, child_idx, writer, .{
                    .indent_level = ctx.indent_level + 1,
                    .in_jsx = true,
                });
                try writer.writeByte('\n');
            }
            try writeIndent(writer, ctx.indent_level);
            try writer.writeAll("</>");
            if (!ctx.in_jsx) {
                try writer.writeByte('\n');
            }
        },

        else => {
            // For unhandled node types, try to preserve source
            const source = ast.nodeSource(node_idx);
            try writer.writeAll(source);
        },
    }
}

fn renderJsxAttributes(ast: *const Ast, node_idx: Ast.NodeIndex, writer: anytype) !void {
    const attrs = ast.jsxAttributes(node_idx);
    for (attrs) |attr| {
        try writer.writeByte(' ');
        const attr_name_raw = ast.tokenSlice(attr.name_token);
        const attr_name = std.mem.trim(u8, attr_name_raw, " \t\n\r");
        try writer.writeAll(attr_name);

        if (attr.value_token.unwrap()) |val_tok| {
            try writer.writeByte('=');
            var val_text = ast.tokenSlice(val_tok);
            val_text = std.mem.trim(u8, val_text, " \t\n\r");

            if (attr.value_type == .expression) {
                try writer.writeByte('{');
                try writer.writeAll(val_text);
                try writer.writeByte('}');
            } else {
                // String literal - ensure quotes
                if (val_text.len >= 2 and val_text[0] == '"' and val_text[val_text.len - 1] == '"') {
                    try writer.writeAll(val_text);
                } else if (val_text.len >= 2 and val_text[0] == '\'' and val_text[val_text.len - 1] == '\'') {
                    // Convert single quotes to double quotes for consistency
                    try writer.writeByte('"');
                    try writer.writeAll(val_text[1 .. val_text.len - 1]);
                    try writer.writeByte('"');
                } else {
                    try writer.writeByte('"');
                    try writer.writeAll(val_text);
                    try writer.writeByte('"');
                }
            }
        }
    }
}

fn extractTokenRangeContent(ast: *const Ast, range: Ast.Node.Range) []const u8 {
    if (range.start >= range.end) return "";

    const token_starts = ast.tokens.items(.start);
    const start = token_starts[range.start];
    const end = if (range.end < ast.tokens.len)
        token_starts[range.end]
    else
        @as(u32, @intCast(ast.source.len));

    return ast.source[start..end];
}

fn extractCodeBlockContent(ast: *const Ast, fence_token: Ast.TokenIndex) []const u8 {
    const token_tags = ast.tokens.items(.tag);
    const token_starts = ast.tokens.items(.start);

    var code_start: u32 = std.math.maxInt(u32);
    var code_end: u32 = 0;
    var in_code = false;

    for (fence_token..ast.tokens.len) |i| {
        const tok_idx: Ast.TokenIndex = @intCast(i);
        if (token_tags[tok_idx] == .code_fence_end) {
            break;
        }
        if (token_tags[tok_idx] == .newline and !in_code) {
            in_code = true;
            continue;
        }
        if (in_code) {
            const start = token_starts[tok_idx];
            const end = if (tok_idx + 1 < ast.tokens.len)
                token_starts[tok_idx + 1]
            else
                @as(u32, @intCast(ast.source.len));

            code_start = @min(code_start, start);
            code_end = @max(code_end, end);
        }
    }

    if (code_start < code_end) {
        return ast.source[code_start..code_end];
    }
    return "";
}

/// Render AST to a newly allocated string
pub fn renderAlloc(ast: *const Ast, allocator: Allocator) ![]u8 {
    var output: std.ArrayList(u8) = .{};
    errdefer output.deinit(allocator);
    try render(ast, &output, allocator);
    return output.toOwnedSlice(allocator);
}

// Tests
test "render simple heading" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "# Hello";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const rendered = try renderAlloc(&ast, allocator);
    defer allocator.free(rendered);

    try std.testing.expectEqualStrings("# Hello\n", rendered);
}

test "render paragraph with bold" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "Hello **world**";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const rendered = try renderAlloc(&ast, allocator);
    defer allocator.free(rendered);

    try std.testing.expectEqualStrings("Hello **world**\n", rendered);
}

test "roundtrip preserves structure" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source =
        \\# Heading
        \\
        \\Paragraph with **bold** and *italic*.
        \\
    ;

    // Parse original
    var ast1 = try mdx.parse(allocator, source);
    defer ast1.deinit(allocator);

    // Render to string
    const rendered = try renderAlloc(&ast1, allocator);
    defer allocator.free(rendered);

    // Parse rendered output
    const rendered_z = try allocator.dupeZ(u8, rendered);
    defer allocator.free(rendered_z);

    var ast2 = try mdx.parse(allocator, rendered_z);
    defer ast2.deinit(allocator);

    // Compare node counts (basic structural check)
    try std.testing.expectEqual(ast1.nodes.len, ast2.nodes.len);
}

test "nodeAtOffset finds correct node" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    // "# Hello" - cursor at offset 2 should find heading, offset 3 should find text
    const source = "# Hello";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    // Offset 0 should be in heading (the # character)
    const at_hash = ast.nodeAtOffset(0);
    try std.testing.expect(at_hash != null);
    if (at_hash) |idx| {
        const node = ast.nodes.get(idx);
        try std.testing.expectEqual(Ast.Node.Tag.heading, node.tag);
    }

    // Offset 2 should be in text node "Hello"
    const at_text = ast.nodeAtOffset(2);
    try std.testing.expect(at_text != null);
    if (at_text) |idx| {
        const node = ast.nodes.get(idx);
        try std.testing.expectEqual(Ast.Node.Tag.text, node.tag);
    }
}

test "nodeSpan returns correct bounds" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "# Hello";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    // Find heading node
    var heading_idx: ?Ast.NodeIndex = null;
    for (0..ast.nodes.len) |i| {
        const idx: Ast.NodeIndex = @intCast(i);
        if (ast.nodes.get(idx).tag == .heading) {
            heading_idx = idx;
            break;
        }
    }

    try std.testing.expect(heading_idx != null);
    if (heading_idx) |idx| {
        const span = ast.nodeSpan(idx);
        try std.testing.expectEqual(@as(Ast.ByteOffset, 0), span.start);
        // Heading should span the whole line
        try std.testing.expect(span.end >= 7);
    }
}

test "render image with alt text" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "![Alt text](image.jpg)";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const rendered = try renderAlloc(&ast, allocator);
    defer allocator.free(rendered);

    try std.testing.expectEqualStrings("![Alt text](image.jpg)\n", rendered);
}

test "render link" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "[Click here](https://example.com)";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const rendered = try renderAlloc(&ast, allocator);
    defer allocator.free(rendered);

    try std.testing.expectEqualStrings("[Click here](https://example.com)\n", rendered);
}

test "render JSX self-closing" {
    const mdx = @import("lib.zig");
    const allocator = std.testing.allocator;

    const source = "<Button label=\"Click\" />";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const rendered = try renderAlloc(&ast, allocator);
    defer allocator.free(rendered);

    try std.testing.expectEqualStrings("<Button label=\"Click\" />\n", rendered);
}
