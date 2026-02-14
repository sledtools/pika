const std = @import("std");
const Ast = @import("Ast.zig");
const Allocator = std.mem.Allocator;

/// TreeBuilder transforms the flat AST into a nested tree structure
/// suitable for JSON serialization and easier consumption.

/// Helper to write JSON-escaped string
fn writeJsonString(writer: anytype, s: []const u8) !void {
    try writer.writeByte('"');
    for (s) |c| {
        switch (c) {
            '"' => try writer.writeAll("\\\""),
            '\\' => try writer.writeAll("\\\\"),
            '\n' => try writer.writeAll("\\n"),
            '\r' => try writer.writeAll("\\r"),
            '\t' => try writer.writeAll("\\t"),
            0x00...0x08, 0x0b, 0x0c, 0x0e...0x1f => try writer.print("\\u{x:0>4}", .{c}),
            else => try writer.writeByte(c),
        }
    }
    try writer.writeByte('"');
}

/// Options for AST serialization
pub const SerializeOptions = struct {
    /// Include byte position info for each node (for cursor mapping in editors)
    include_positions: bool = false,
};

/// Serialize the AST as a nested tree structure to JSON
pub fn serializeTree(ast: *const Ast, output: *std.ArrayList(u8), allocator: Allocator) !void {
    return serializeTreeWithOptions(ast, output, allocator, .{});
}

/// Serialize the AST with options (e.g., include position info for cursor mapping)
pub fn serializeTreeWithOptions(ast: *const Ast, output: *std.ArrayList(u8), allocator: Allocator, options: SerializeOptions) !void {
    const writer = output.writer(allocator);

    try writer.writeAll("{\"type\":\"root\",\"children\":[");

    // Find the document node (usually the last node)
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
        for (children, 0..) |child_idx, i| {
            if (i > 0) try writer.writeAll(",");
            try serializeNodeWithOptions(ast, child_idx, writer, options);
        }
    }

    try writer.writeAll("],\"source\":");
    try writeJsonString(writer, ast.source);

    // Include errors
    try writer.writeAll(",\"errors\":[");
    for (ast.errors, 0..) |err, i| {
        if (i > 0) try writer.writeAll(",");
        try writer.writeAll("{");
        try writer.print("\"tag\":\"{s}\",", .{@tagName(err.tag)});
        try writer.print("\"token\":{d}", .{err.token});
        try writer.writeAll("}");
    }
    try writer.writeAll("]}");
}

fn serializeNodeWithOptions(ast: *const Ast, node_idx: Ast.NodeIndex, writer: anytype, options: SerializeOptions) !void {
    const node = ast.nodes.get(node_idx);

    try writer.writeAll("{");
    try writer.print("\"type\":\"{s}\"", .{@tagName(node.tag)});

    // Add position info for cursor mapping (optional)
    if (options.include_positions) {
        const span = ast.nodeSpan(node_idx);
        try writer.print(",\"position\":{{\"start\":{d},\"end\":{d}}}", .{ span.start, span.end });
    }

    switch (node.tag) {
        .heading => {
            const info = ast.headingInfo(node_idx);
            try writer.print(",\"level\":{d}", .{info.level});
            try writer.writeAll(",\"children\":[");
            const children_indices = @as(
                []const Ast.NodeIndex,
                @ptrCast(ast.extra_data[info.children_start..info.children_end]),
            );
            for (children_indices, 0..) |child_idx, i| {
                if (i > 0) try writer.writeAll(",");
                try serializeNodeWithOptions(ast, child_idx, writer, options);
            }
            try writer.writeAll("]");
        },

        .text => {
            const text = ast.tokenSlice(node.main_token);
            try writer.writeAll(",\"value\":");
            try writeJsonString(writer, text);
        },

        .code_block => {
            // Extract language from the token after code_fence_start
            const fence_token = node.main_token;
            const token_tags = ast.tokens.items(.tag);

            // Check if there's a language token after the fence
            var lang: ?[]const u8 = null;
            if (fence_token + 1 < ast.tokens.len) {
                const next_token = fence_token + 1;
                // If the next token is text (not newline), it's the language
                if (token_tags[next_token] == .text) {
                    const lang_text = ast.tokenSlice(next_token);
                    const trimmed = std.mem.trim(u8, lang_text, " \t\n\r");
                    if (trimmed.len > 0) {
                        lang = trimmed;
                    }
                }
            }

            if (lang) |l| {
                try writer.writeAll(",\"lang\":");
                try writeJsonString(writer, l);
            }

            // Get the code content (tokens between fence_start and fence_end)
            var code_start: u32 = std.math.maxInt(u32);
            var code_end: u32 = 0;

            const token_starts = ast.tokens.items(.start);

            // Find the range of content tokens
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

            const code = if (code_start < code_end)
                ast.source[code_start..code_end]
            else
                "";

            try writer.writeAll(",\"value\":");
            try writeJsonString(writer, code);
        },

        .code_inline => {
            const content_token = node.data.token;
            const text = ast.tokenSlice(content_token);
            try writer.writeAll(",\"value\":");
            try writeJsonString(writer, text);
        },

        .link, .image => {
            const link = ast.extraData(node.data.extra, Ast.Link);
            const url = ast.tokenSlice(link.url_token);

            try writer.writeAll(",\"url\":");
            try writeJsonString(writer, url);

            // Serialize the text node as children
            if (link.text_node.unwrap()) |text_idx| {
                try writer.writeAll(",\"children\":[");
                try serializeNodeWithOptions(ast, text_idx, writer, options);
                try writer.writeAll("]");
            } else {
                try writer.writeAll(",\"children\":[]");
            }
        },

        .mdx_jsx_element, .mdx_jsx_self_closing => {
            const elem = ast.jsxElement(node_idx);
            const name_raw = ast.tokenSlice(elem.name_token);
            const name = std.mem.trim(u8, name_raw, " \t\n\r");

            try writer.writeAll(",\"name\":");
            try writeJsonString(writer, name);

            // Serialize attributes
            try writer.writeAll(",\"attributes\":[");
            const attrs = ast.jsxAttributes(node_idx);
            for (attrs, 0..) |attr, i| {
                if (i > 0) try writer.writeAll(",");
                try writer.writeAll("{");

                const attr_name_raw = ast.tokenSlice(attr.name_token);
                const attr_name = std.mem.trim(u8, attr_name_raw, " \t\n\r");
                try writer.writeAll("\"name\":");
                try writeJsonString(writer, attr_name);

                // Add attribute type
                try writer.writeAll(",\"type\":\"");
                try writer.writeAll(@tagName(attr.value_type));
                try writer.writeAll("\"");

                if (attr.value_token.unwrap()) |val_tok| {
                    const val_text_raw = ast.tokenSlice(val_tok);
                    var val_text = std.mem.trim(u8, val_text_raw, " \t\n\r");

                    // Strip quotes from string literals only
                    if (attr.value_type == .literal and val_text.len >= 2 and
                        ((val_text[0] == '"' and val_text[val_text.len - 1] == '"') or
                        (val_text[0] == '\'' and val_text[val_text.len - 1] == '\'')))
                    {
                        val_text = val_text[1 .. val_text.len - 1];
                    }

                    try writer.writeAll(",\"value\":");
                    try writeJsonString(writer, val_text);
                }

                try writer.writeAll("}");
            }
            try writer.writeAll("]");

            // Serialize children if present
            if (node.tag == .mdx_jsx_element) {
                try writer.writeAll(",\"children\":[");
                const children_indices = @as(
                    []const Ast.NodeIndex,
                    @ptrCast(ast.extra_data[elem.children_start..elem.children_end]),
                );
                for (children_indices, 0..) |child_idx, i| {
                    if (i > 0) try writer.writeAll(",");
                    try serializeNodeWithOptions(ast, child_idx, writer, options);
                }
                try writer.writeAll("]");
            }
        },

        .frontmatter => {
            const range = ast.extraData(node.data.extra, Ast.Node.Range);

            // Extract frontmatter content from tokens
            var fm_start: u32 = std.math.maxInt(u32);
            var fm_end: u32 = 0;

            const token_starts = ast.tokens.items(.start);
            for (range.start..range.end) |i| {
                const tok_idx: Ast.TokenIndex = @intCast(i);
                const start = token_starts[tok_idx];
                const end = if (tok_idx + 1 < ast.tokens.len)
                    token_starts[tok_idx + 1]
                else
                    @as(u32, @intCast(ast.source.len));

                fm_start = @min(fm_start, start);
                fm_end = @max(fm_end, end);
            }

            const content = if (fm_start < fm_end)
                std.mem.trim(u8, ast.source[fm_start..fm_end], " \t\n\r")
            else
                "";

            try writer.writeAll(",\"value\":");
            try writeJsonString(writer, content);
        },

        .mdx_text_expression, .mdx_flow_expression => {
            const range = ast.extraData(node.data.extra, Ast.Node.Range);

            // Extract expression content from tokens
            var expr_start: u32 = std.math.maxInt(u32);
            var expr_end: u32 = 0;

            const token_starts = ast.tokens.items(.start);
            for (range.start..range.end) |i| {
                const tok_idx: Ast.TokenIndex = @intCast(i);
                const start = token_starts[tok_idx];
                const end = if (tok_idx + 1 < ast.tokens.len)
                    token_starts[tok_idx + 1]
                else
                    @as(u32, @intCast(ast.source.len));

                expr_start = @min(expr_start, start);
                expr_end = @max(expr_end, end);
            }

            const content = if (expr_start < expr_end)
                std.mem.trim(u8, ast.source[expr_start..expr_end], " \t\n\r")
            else
                "";

            try writer.writeAll(",\"value\":");
            try writeJsonString(writer, content);
        },

        // Nodes with children arrays
        .document,
        .paragraph,
        .blockquote,
        .list_unordered,
        .list_ordered,
        .list_item,
        .strong,
        .emphasis,
        .mdx_jsx_fragment,
        => {
            try writer.writeAll(",\"children\":[");
            const children = ast.children(node_idx);
            for (children, 0..) |child_idx, i| {
                if (i > 0) try writer.writeAll(",");
                try serializeNodeWithOptions(ast, child_idx, writer, options);
            }
            try writer.writeAll("]");
        },

        .hr => {
            // No additional data
        },

        else => {
            // Unknown node type - just output type
        },
    }

    try writer.writeAll("}");
}
