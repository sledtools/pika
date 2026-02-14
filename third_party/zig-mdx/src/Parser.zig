const std = @import("std");
const Allocator = std.mem.Allocator;
const Token = @import("Token.zig");
const Tokenizer = @import("Tokenizer.zig");
const Ast = @import("Ast.zig");

gpa: Allocator,
source: [:0]const u8,
token_tags: []const Token.Tag,
token_starts: []const Ast.ByteOffset,
token_index: Ast.TokenIndex,
nodes: Ast.NodeList,
extra_data: std.ArrayList(u32),
scratch: std.ArrayList(Ast.NodeIndex),
errors: std.ArrayList(Ast.Error),

const Parser = @This();

pub fn parse(gpa: Allocator, source: [:0]const u8) !Ast {
    // Phase 1: Tokenization
    var tokens: std.ArrayList(Token) = .{};
    defer tokens.deinit(gpa);

    var tokenizer = Tokenizer.init(source, gpa);
    defer tokenizer.deinit();

    // Estimate capacity based on source length (empirical ratio: 8:1)
    const estimated_token_count = @max(source.len / 8, 16);
    try tokens.ensureTotalCapacity(gpa, estimated_token_count);

    while (true) {
        const tok = tokenizer.next();
        try tokens.append(gpa, tok);
        if (tok.tag == .eof) break;
    }


    // Phase 2: Parsing
    var parser = Parser.init(gpa, source, tokens.items);
    defer parser.deinitExceptNodes();

    // Estimate node count (empirical ratio: 2:1 tokens to nodes)
    const estimated_node_count = @max(tokens.items.len / 2, 8);
    try parser.nodes.ensureTotalCapacity(gpa, estimated_node_count);
    try parser.extra_data.ensureTotalCapacity(gpa, estimated_node_count * 2);

    const root_node = try parser.parseDocument();
    _ = root_node; // Root is always at index 0

    // Build final AST
    var token_list = Ast.TokenList{};
    try token_list.ensureTotalCapacity(gpa, tokens.items.len);
    for (tokens.items) |tok| {
        token_list.appendAssumeCapacity(.{
            .tag = tok.tag,
            .start = tok.loc.start,
        });
    }

    return Ast{
        .source = source,
        .tokens = token_list,
        .nodes = parser.nodes,
        .extra_data = try parser.extra_data.toOwnedSlice(gpa),
        .errors = try parser.errors.toOwnedSlice(gpa),
    };
}

fn init(gpa: Allocator, source: [:0]const u8, tokens: []const Token) Parser {
    var token_tags = gpa.alloc(Token.Tag, tokens.len) catch @panic("OOM");
    var token_starts = gpa.alloc(Ast.ByteOffset, tokens.len) catch @panic("OOM");

    for (tokens, 0..) |tok, i| {
        token_tags[i] = tok.tag;
        token_starts[i] = tok.loc.start;
    }

    return .{
        .gpa = gpa,
        .source = source,
        .token_tags = token_tags,
        .token_starts = token_starts,
        .token_index = 0,
        .nodes = .{},
        .extra_data = .{},
        .scratch = .{},
        .errors = .{},
    };
}

fn deinit(p: *Parser) void {
    p.gpa.free(p.token_tags);
    p.gpa.free(p.token_starts);
    p.nodes.deinit(p.gpa);
    p.extra_data.deinit();
    p.scratch.deinit();
    p.errors.deinit();
}

fn deinitExceptNodes(p: *Parser) void {
    p.gpa.free(p.token_tags);
    p.gpa.free(p.token_starts);
    // DON'T deinit nodes - they've been moved to the AST
    p.scratch.deinit(p.gpa);
    // DON'T deinit extra_data or errors - they've been converted to owned slices
}

// === Token consumption methods ===

fn eatToken(p: *Parser, tag: Token.Tag) ?Ast.TokenIndex {
    if (p.token_tags[p.token_index] == tag) {
        const result = p.token_index;
        p.token_index += 1;
        return result;
    }
    return null;
}

fn expectToken(p: *Parser, tag: Token.Tag) !Ast.TokenIndex {
    if (p.eatToken(tag)) |idx| {
        return idx;
    }
    try p.warn(.expected_token);
    return error.ParseError;
}

fn nextToken(p: *Parser) Ast.TokenIndex {
    const result = p.token_index;
    p.token_index += 1;
    return result;
}

fn peekToken(p: *Parser, offset: u32) Token.Tag {
    const index = p.token_index + offset;
    if (index >= p.token_tags.len) return .eof;
    return p.token_tags[index];
}

// === Node creation methods ===

fn addNode(p: *Parser, node: Ast.Node) !Ast.NodeIndex {
    const index: Ast.NodeIndex = @intCast(p.nodes.len);
    try p.nodes.append(p.gpa, node);
    return index;
}

fn reserveNode(p: *Parser, tag: Ast.Node.Tag) !Ast.NodeIndex {
    const index: Ast.NodeIndex = @intCast(p.nodes.len);
    try p.nodes.append(p.gpa, .{
        .tag = tag,
        .main_token = 0,
        .data = .{ .none = {} },
    });
    return index;
}

fn setNode(p: *Parser, index: Ast.NodeIndex, node: Ast.Node) Ast.NodeIndex {
    p.nodes.set(index, node);
    return index;
}

fn unreserveNode(p: *Parser, index: Ast.NodeIndex) void {
    if (index == p.nodes.len - 1) {
        _ = p.nodes.pop();
    }
}

// === Extra data methods ===

fn addExtra(p: *Parser, value: anytype) !u32 {
    const T = @TypeOf(value);
    const fields = @typeInfo(T).@"struct".fields;
    const start: u32 = @intCast(p.extra_data.items.len);

    try p.extra_data.ensureUnusedCapacity(p.gpa, fields.len);
    inline for (fields) |field| {
        const field_value = @field(value, field.name);
        // Handle different field sizes
        const as_u32: u32 = switch (@typeInfo(field.type)) {
            .int => @intCast(field_value),
            .@"enum" => @intFromEnum(field_value),
            else => @bitCast(field_value),
        };
        p.extra_data.appendAssumeCapacity(as_u32);
    }

    return start;
}

fn listToSpan(p: *Parser, items: []const Ast.NodeIndex) !Ast.Node.Range {
    const start: u32 = @intCast(p.extra_data.items.len);
    try p.extra_data.appendSlice(p.gpa, @as([]const u32, @ptrCast(items)));
    return .{ .start = start, .end = @intCast(p.extra_data.items.len) };
}

// === Error handling ===

fn warn(p: *Parser, tag: Ast.Error.Tag) !void {
    try p.errors.append(p.gpa, .{
        .tag = tag,
        .token = p.token_index,
    });
}

fn findNextBlock(p: *Parser) void {
    while (p.token_index < p.token_tags.len) {
        switch (p.token_tags[p.token_index]) {
            .eof, .blank_line, .heading_start, .hr, .frontmatter_start => return,
            else => p.token_index += 1,
        }
    }
}

// === Parsing methods ===

fn parseDocument(p: *Parser) !Ast.NodeIndex {
    const scratch_top = p.scratch.items.len;
    defer p.scratch.shrinkRetainingCapacity(scratch_top);

    // Check for frontmatter
    if (p.eatToken(.frontmatter_start)) |fm_start| {
        const fm_node = try p.parseFrontmatter(fm_start);
        try p.scratch.append(p.gpa, fm_node);
    }

    // Parse top-level blocks
    while (p.token_tags[p.token_index] != .eof) {
        // Skip newlines and blank lines between blocks
        while (p.token_tags[p.token_index] == .blank_line or
            p.token_tags[p.token_index] == .newline)
        {
            p.token_index += 1;
        }

        if (p.token_tags[p.token_index] == .eof) break;

        const block = p.parseBlock() catch |err| {
            if (err == error.ParseError) {
                p.findNextBlock();
                continue;
            }
            return err;
        };

        try p.scratch.append(p.gpa, block);
    }

    const children_span = try p.listToSpan(p.scratch.items[scratch_top..]);

    return p.addNode(.{
        .tag = .document,
        .main_token = 0,
        .data = .{ .children = children_span },
    });
}

fn parseFrontmatter(p: *Parser, start_token: Ast.TokenIndex) !Ast.NodeIndex {
    // Skip newline after ---
    _ = p.eatToken(.newline);

    // Consume content until closing ---
    const content_start = p.token_index;
    while (p.token_tags[p.token_index] != .hr and
        p.token_tags[p.token_index] != .eof)
    {
        p.token_index += 1;
    }
    const content_end = p.token_index;

    // Expect closing ---
    if (p.token_tags[p.token_index] != .hr) {
        try p.warn(.unclosed_frontmatter);
        return error.ParseError;
    }
    _ = p.nextToken(); // consume hr

    const range_index = try p.addExtra(Ast.Node.Range{
        .start = content_start,
        .end = content_end,
    });

    return p.addNode(.{
        .tag = .frontmatter,
        .main_token = start_token,
        .data = .{ .extra = range_index },
    });
}

fn parseBlock(p: *Parser) error{ OutOfMemory, ParseError }!Ast.NodeIndex {
    return switch (p.token_tags[p.token_index]) {
        .heading_start => p.parseHeading(),
        .code_fence_start => p.parseCodeBlock(),
        .hr => p.parseHr(),
        .blockquote_start => p.parseBlockquote(),
        .list_item_unordered, .list_item_ordered => p.parseList(),
        .jsx_tag_start => p.parseJsxElement(),
        // expr_start falls through to parseParagraph - expressions are inline content
        // This allows {expr}  \n{expr2} to be parsed as one paragraph with hard break
        else => p.parseParagraph(),
    };
}

fn parseHeading(p: *Parser) !Ast.NodeIndex {
    const heading_token = p.nextToken();

    // Count # characters to determine level
    const heading_text = p.tokenSlice(heading_token);
    var level: u8 = 0;
    for (heading_text) |ch| {
        if (ch == '#') level += 1 else break;
    }

    // Reserve node for children
    const node_index = try p.reserveNode(.heading);

    // Parse inline content - if this fails, we still need to set the node
    const children_span = p.parseInlineContent(.newline) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        const empty_heading = try p.addExtra(Ast.Heading{
            .level = level,
            .children_start = 0,
            .children_end = 0,
        });
        _ = p.setNode(node_index, .{
            .tag = .heading,
            .main_token = heading_token,
            .data = .{ .extra = empty_heading },
        });
        return err;
    };

    const heading_index = try p.addExtra(Ast.Heading{
        .level = level,
        .children_start = children_span.start,
        .children_end = children_span.end,
    });

    return p.setNode(node_index, .{
        .tag = .heading,
        .main_token = heading_token,
        .data = .{ .extra = heading_index },
    });
}

fn parseParagraph(p: *Parser) !Ast.NodeIndex {
    const start_token = p.token_index;

    // Reserve node
    const node_index = try p.reserveNode(.paragraph);

    // Parse inline content until blank line (double newline)
    const children_span = p.parseInlineContent(.blank_line) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        _ = p.setNode(node_index, .{
            .tag = .paragraph,
            .main_token = start_token,
            .data = .{ .children = .{ .start = 0, .end = 0 } },
        });
        return err;
    };

    return p.setNode(node_index, .{
        .tag = .paragraph,
        .main_token = start_token,
        .data = .{ .children = children_span },
    });
}

fn parseInlineContent(p: *Parser, end_tag: Token.Tag) error{ OutOfMemory, ParseError }!Ast.Node.Range {
    const scratch_top = p.scratch.items.len;
    defer p.scratch.shrinkRetainingCapacity(scratch_top);

    while (p.token_tags[p.token_index] != end_tag and
        p.token_tags[p.token_index] != .eof and
        p.token_tags[p.token_index] != .blank_line)
    {
        // Skip newlines within inline content (soft breaks)
        if (p.token_tags[p.token_index] == .newline) {
            _ = p.nextToken();
            continue;
        }

        const inline_node = try p.parseInline();
        try p.scratch.append(p.gpa, inline_node);
    }

    _ = p.eatToken(end_tag); // consume end token

    return p.listToSpan(p.scratch.items[scratch_top..]);
}

fn parseInline(p: *Parser) !Ast.NodeIndex {
    return switch (p.token_tags[p.token_index]) {
        .text => p.parseText(),
        .strong_start => p.parseStrong(),
        .emphasis_start => p.parseEmphasis(),
        .code_inline_start => p.parseCodeInline(),
        .link_start => p.parseLink(),
        .image_start => p.parseImage(),
        .hard_break => p.parseHardBreak(),
        .expr_start => p.parseTextExpression(),
        .jsx_tag_start => p.parseJsxElement(),
        else => {
            try p.warn(.unexpected_token);
            _ = p.nextToken();
            return error.ParseError;
        },
    };
}

fn parseText(p: *Parser) !Ast.NodeIndex {
    const text_token = p.nextToken();
    return p.addNode(.{
        .tag = .text,
        .main_token = text_token,
        .data = .{ .none = {} },
    });
}

fn parseHardBreak(p: *Parser) !Ast.NodeIndex {
    const break_token = p.nextToken();
    return p.addNode(.{
        .tag = .hard_break,
        .main_token = break_token,
        .data = .{ .none = {} },
    });
}

fn parseStrong(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // **

    const node_index = try p.reserveNode(.strong);

    const children_span = p.parseInlineContent(.strong_end) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        _ = p.setNode(node_index, .{
            .tag = .strong,
            .main_token = start_token,
            .data = .{ .children = .{ .start = 0, .end = 0 } },
        });
        return err;
    };

    return p.setNode(node_index, .{
        .tag = .strong,
        .main_token = start_token,
        .data = .{ .children = children_span },
    });
}

fn parseEmphasis(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // *

    const node_index = try p.reserveNode(.emphasis);

    const children_span = p.parseInlineContent(.emphasis_end) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        _ = p.setNode(node_index, .{
            .tag = .emphasis,
            .main_token = start_token,
            .data = .{ .children = .{ .start = 0, .end = 0 } },
        });
        return err;
    };

    return p.setNode(node_index, .{
        .tag = .emphasis,
        .main_token = start_token,
        .data = .{ .children = children_span },
    });
}

fn parseCodeInline(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // `

    _ = try p.expectToken(.text); // code content
    _ = try p.expectToken(.code_inline_end); // `

    return p.addNode(.{
        .tag = .code_inline,
        .main_token = start_token,
        .data = .{ .token = start_token + 1 },
    });
}

fn parseLink(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // [

    // Parse link text
    const text_node: Ast.OptionalNodeIndex = if (p.token_tags[p.token_index] == .text)
        Ast.OptionalNodeIndex.init(try p.parseText())
    else
        .none;

    _ = try p.expectToken(.link_end); // ]
    _ = try p.expectToken(.link_url_start); // (

    const url_token = try p.expectToken(.text);

    _ = try p.expectToken(.link_url_end); // )

    const link_data = try p.addExtra(Ast.Link{
        .text_node = text_node,
        .url_token = url_token,
    });

    return p.addNode(.{
        .tag = .link,
        .main_token = start_token,
        .data = .{ .extra = link_data },
    });
}

fn parseImage(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // ![

    // Parse alt text (same as link text)
    const text_node: Ast.OptionalNodeIndex = if (p.token_tags[p.token_index] == .text)
        Ast.OptionalNodeIndex.init(try p.parseText())
    else
        .none;

    _ = try p.expectToken(.link_end); // ]
    _ = try p.expectToken(.link_url_start); // (

    const url_token = try p.expectToken(.text);

    _ = try p.expectToken(.link_url_end); // )

    const link_data = try p.addExtra(Ast.Link{
        .text_node = text_node,
        .url_token = url_token,
    });

    return p.addNode(.{
        .tag = .image,
        .main_token = start_token,
        .data = .{ .extra = link_data },
    });
}

fn parseCodeBlock(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // ```

    // Optional language identifier
    _ = p.eatToken(.text);
    _ = p.eatToken(.newline);

    // Consume until closing ```
    while (p.token_tags[p.token_index] != .code_fence_end and
        p.token_tags[p.token_index] != .eof)
    {
        p.token_index += 1;
    }

    _ = try p.expectToken(.code_fence_end);

    return p.addNode(.{
        .tag = .code_block,
        .main_token = start_token,
        .data = .{ .none = {} },
    });
}

fn parseHr(p: *Parser) !Ast.NodeIndex {
    const hr_token = p.nextToken();
    return p.addNode(.{
        .tag = .hr,
        .main_token = hr_token,
        .data = .{ .none = {} },
    });
}

fn parseBlockquote(p: *Parser) !Ast.NodeIndex {
    const start_token = p.nextToken(); // >

    const node_index = try p.reserveNode(.blockquote);

    // Skip space after >
    _ = p.eatToken(.space);

    const children_span = p.parseInlineContent(.newline) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        _ = p.setNode(node_index, .{
            .tag = .blockquote,
            .main_token = start_token,
            .data = .{ .children = .{ .start = 0, .end = 0 } },
        });
        return err;
    };

    return p.setNode(node_index, .{
        .tag = .blockquote,
        .main_token = start_token,
        .data = .{ .children = children_span },
    });
}

fn parseList(p: *Parser) !Ast.NodeIndex {
    const first_item_tag = p.token_tags[p.token_index];
    const list_tag: Ast.Node.Tag = if (first_item_tag == .list_item_ordered)
        .list_ordered
    else
        .list_unordered;

    const start_token = p.token_index;
    const node_index = try p.reserveNode(list_tag);

    const scratch_top = p.scratch.items.len;
    defer p.scratch.shrinkRetainingCapacity(scratch_top);

    while (p.token_tags[p.token_index] == first_item_tag) {
        const item = p.parseListItem() catch |err| {
            // On error, set node with empty children
            _ = p.setNode(node_index, .{
                .tag = list_tag,
                .main_token = start_token,
                .data = .{ .children = .{ .start = 0, .end = 0 } },
            });
            return err;
        };
        try p.scratch.append(p.gpa, item);
    }

    const children_span = try p.listToSpan(p.scratch.items[scratch_top..]);

    return p.setNode(node_index, .{
        .tag = list_tag,
        .main_token = start_token,
        .data = .{ .children = children_span },
    });
}

fn parseListItem(p: *Parser) !Ast.NodeIndex {
    const item_token = p.nextToken();

    const node_index = try p.reserveNode(.list_item);

    const children_span = p.parseInlineContent(.newline) catch |err| {
        // Set node with empty children to avoid leaving incomplete node
        _ = p.setNode(node_index, .{
            .tag = .list_item,
            .main_token = item_token,
            .data = .{ .children = .{ .start = 0, .end = 0 } },
        });
        return err;
    };

    return p.setNode(node_index, .{
        .tag = .list_item,
        .main_token = item_token,
        .data = .{ .children = children_span },
    });
}

fn parseTextExpression(p: *Parser) !Ast.NodeIndex {
    const expr_start = try p.expectToken(.expr_start);

    // Content until }
    const content_start = p.token_index;
    var depth: u32 = 1;

    while (depth > 0 and p.token_tags[p.token_index] != .eof) {
        switch (p.token_tags[p.token_index]) {
            .expr_start => depth += 1,
            .expr_end => depth -= 1,
            else => {},
        }
        if (depth > 0) p.token_index += 1;
    }

    const content_end = p.token_index;

    _ = try p.expectToken(.expr_end);

    const range_index = try p.addExtra(Ast.Node.Range{
        .start = content_start,
        .end = content_end,
    });

    return p.addNode(.{
        .tag = .mdx_text_expression,
        .main_token = expr_start,
        .data = .{ .extra = range_index },
    });
}

fn parseFlowExpression(p: *Parser) !Ast.NodeIndex {
    const expr_start = try p.expectToken(.expr_start);

    // Content until }
    const content_start = p.token_index;
    var depth: u32 = 1;

    while (depth > 0 and p.token_tags[p.token_index] != .eof) {
        switch (p.token_tags[p.token_index]) {
            .expr_start => depth += 1,
            .expr_end => depth -= 1,
            else => {},
        }
        if (depth > 0) p.token_index += 1;
    }

    const content_end = p.token_index;

    _ = try p.expectToken(.expr_end);

    const range_index = try p.addExtra(Ast.Node.Range{
        .start = content_start,
        .end = content_end,
    });

    return p.addNode(.{
        .tag = .mdx_flow_expression, // Block-level expression
        .main_token = expr_start,
        .data = .{ .extra = range_index },
    });
}

fn parseJsxElement(p: *Parser) !Ast.NodeIndex {
    const open_bracket = try p.expectToken(.jsx_tag_start);

    // Check for closing tag
    if (p.eatToken(.jsx_close_tag)) |_| {
        return p.parseJsxClosingTag();
    }

    // Check for fragment
    if (p.peekToken(0) == .jsx_tag_end) {
        return p.parseJsxFragment();
    }

    const name = try p.expectToken(.jsx_identifier);

    // Parse attributes
    const attrs_start: u32 = @intCast(p.extra_data.items.len);
    while (p.token_tags[p.token_index] == .jsx_identifier) {
        const attr_name = p.nextToken();

        // Optional = and value
        var attr_value: Ast.OptionalTokenIndex = .none;
        var attr_type: Ast.JsxAttributeType = .literal;
        if (p.eatToken(.jsx_equal)) |_| {
            if (p.eatToken(.jsx_string)) |val| {
                attr_value = Ast.OptionalTokenIndex.init(val);
                attr_type = .literal;
            } else if (p.eatToken(.jsx_attr_expr_start)) |_| {
                // Parse expression value - consume tokens until expr_end
                const expr_content_start = p.token_index;
                while (p.token_tags[p.token_index] != .expr_end and
                    p.token_tags[p.token_index] != .eof)
                {
                    p.token_index += 1;
                }
                _ = try p.expectToken(.expr_end);
                attr_value = Ast.OptionalTokenIndex.init(expr_content_start);
                attr_type = .expression;
            }
        }

        // Add attribute to extra_data (3 u32s per attribute)
        try p.extra_data.append(p.gpa, attr_name);
        try p.extra_data.append(p.gpa, @intFromEnum(attr_value));
        try p.extra_data.append(p.gpa, @intFromEnum(attr_type));
    }
    const attrs_end: u32 = @intCast(p.extra_data.items.len);

    // Check for self-closing
    if (p.eatToken(.jsx_self_close)) |_| {
        const jsx_data = try p.addExtra(Ast.JsxElement{
            .name_token = name,
            .attrs_start = attrs_start,
            .attrs_end = attrs_end,
            .children_start = 0,
            .children_end = 0,
        });

        return p.addNode(.{
            .tag = .mdx_jsx_self_closing,
            .main_token = open_bracket,
            .data = .{ .extra = jsx_data },
        });
    }

    _ = try p.expectToken(.jsx_tag_end);

    // Parse children - handle text, expressions, and nested JSX
    const scratch_top = p.scratch.items.len;
    defer p.scratch.shrinkRetainingCapacity(scratch_top);

    // Parse inline content until we hit closing tag
    while (p.token_tags[p.token_index] != .jsx_close_tag and
        p.token_tags[p.token_index] != .eof)
    {
        const tag = p.token_tags[p.token_index];

        switch (tag) {
            .jsx_tag_start => {
                // Check if it's a closing tag
                if (p.peekToken(1) == .jsx_close_tag) {
                    break;
                }
                // Parse nested JSX element
                const child = try p.parseBlock();
                try p.scratch.append(p.gpa, child);
            },
            .expr_start => {
                // Parse MDX expression
                const child = try p.parseTextExpression();
                try p.scratch.append(p.gpa, child);
            },
            .text => {
                // Parse text node
                const child = try p.parseText();
                try p.scratch.append(p.gpa, child);
            },
            .code_inline_start => {
                // Parse inline code
                const child = try p.parseCodeInline();
                try p.scratch.append(p.gpa, child);
            },
            .strong_start => {
                // Parse strong (bold)
                const child = try p.parseStrong();
                try p.scratch.append(p.gpa, child);
            },
            .emphasis_start => {
                // Parse emphasis (italic)
                const child = try p.parseEmphasis();
                try p.scratch.append(p.gpa, child);
            },
            .link_start => {
                // Parse link
                const child = try p.parseLink();
                try p.scratch.append(p.gpa, child);
            },
            .image_start => {
                // Parse image
                const child = try p.parseImage();
                try p.scratch.append(p.gpa, child);
            },
            .hard_break => {
                // Parse hard break
                const child = try p.parseHardBreak();
                try p.scratch.append(p.gpa, child);
            },
            .heading_start => {
                // Parse heading inside JSX
                const child = try p.parseHeading();
                try p.scratch.append(p.gpa, child);
            },
            .newline, .blank_line => {
                // Skip whitespace between children
                p.token_index += 1;
            },
            else => {
                // Skip unknown token
                p.token_index += 1;
            },
        }
    }

    const children_span = try p.listToSpan(p.scratch.items[scratch_top..]);

    // Expect closing tag
    _ = try p.expectToken(.jsx_close_tag);
    _ = try p.expectToken(.jsx_identifier); // TODO: verify matching
    _ = try p.expectToken(.jsx_tag_end);

    const jsx_data = try p.addExtra(Ast.JsxElement{
        .name_token = name,
        .attrs_start = attrs_start,
        .attrs_end = attrs_end,
        .children_start = children_span.start,
        .children_end = children_span.end,
    });

    return p.addNode(.{
        .tag = .mdx_jsx_element,
        .main_token = open_bracket,
        .data = .{ .extra = jsx_data },
    });
}

fn parseJsxClosingTag(p: *Parser) !Ast.NodeIndex {
    _ = try p.expectToken(.jsx_identifier);
    _ = try p.expectToken(.jsx_tag_end);
    return error.ParseError; // Closing tags shouldn't appear at block level
}

fn parseJsxFragment(p: *Parser) !Ast.NodeIndex {
    const open_bracket = p.token_index - 1; // jsx_tag_start
    _ = try p.expectToken(.jsx_tag_end); // >

    const scratch_top = p.scratch.items.len;
    defer p.scratch.shrinkRetainingCapacity(scratch_top);

    // Parse children until </>
    while (!(p.token_tags[p.token_index] == .jsx_tag_start and
        p.peekToken(1) == .jsx_close_tag))
    {
        if (p.token_tags[p.token_index] == .eof) {
            try p.warn(.expected_closing_tag);
            return error.ParseError;
        }
        const child = try p.parseBlock();
        try p.scratch.append(p.gpa, child);
    }

    const children_span = try p.listToSpan(p.scratch.items[scratch_top..]);

    // Expect </>
    _ = try p.expectToken(.jsx_tag_start);
    _ = try p.expectToken(.jsx_close_tag);
    _ = try p.expectToken(.jsx_tag_end);

    return p.addNode(.{
        .tag = .mdx_jsx_fragment,
        .main_token = open_bracket,
        .data = .{ .children = children_span },
    });
}

fn tokenSlice(p: *Parser, token_index: Ast.TokenIndex) []const u8 {
    const start = p.token_starts[token_index];
    const end = if (token_index + 1 < p.token_starts.len)
        p.token_starts[token_index + 1]
    else
        @as(u32, @intCast(p.source.len));
    return p.source[start..end];
}

// Tests
test "parse heading" {
    const source = "# Hello World\n";
    var tree = try parse(std.testing.allocator, source);
    defer tree.deinit(std.testing.allocator);

    // Should have at least the heading node
    try std.testing.expect(tree.nodes.len >= 1);

    // Find the heading node
    var heading_idx: ?Ast.NodeIndex = null;
    for (0..tree.nodes.len) |i| {
        if (tree.nodes.get(@intCast(i)).tag == .heading) {
            heading_idx = @intCast(i);
            break;
        }
    }

    try std.testing.expect(heading_idx != null);

    if (heading_idx) |idx| {
        const heading_info = tree.headingInfo(idx);
        try std.testing.expectEqual(@as(u8, 1), heading_info.level);
    }
}

test "parse paragraph with expression" {
    const source = "Hello {name}\n";
    var tree = try parse(std.testing.allocator, source);
    defer tree.deinit(std.testing.allocator);

    // Should have at least one paragraph node
    var found_paragraph = false;
    for (0..tree.nodes.len) |i| {
        if (tree.nodes.get(@intCast(i)).tag == .paragraph) {
            found_paragraph = true;
            break;
        }
    }

    try std.testing.expect(found_paragraph);
}
