const std = @import("std");
const mdx = @import("lib.zig");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // Get command line arguments
    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);

    if (args.len < 2) {
        var stderr_buffer: [1024]u8 = undefined;
        var stderr_fbs = std.io.fixedBufferStream(&stderr_buffer);
        const stderr_writer = stderr_fbs.writer();
        try stderr_writer.print("Usage: {s} <file.hnmd>\n", .{args[0]});
        _ = try std.fs.File.stderr().writeAll(stderr_fbs.getWritten());
        std.process.exit(1);
    }

    const filename = args[1];

    // Read file
    const file = try std.fs.cwd().openFile(filename, .{});
    defer file.close();

    const source = try file.readToEndAllocOptions(allocator, std.math.maxInt(usize), null, .@"1", 0);
    defer allocator.free(source);

    // Parse
    var stdout_buffer: [16384]u8 = undefined;
    var stdout_fbs = std.io.fixedBufferStream(&stdout_buffer);
    const stdout = stdout_fbs.writer();
    try stdout.print("Parsing: {s}\n\n", .{filename});

    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    // Print AST
    try printAst(stdout, ast);

    // Print errors if any
    if (ast.errors.len > 0) {
        try stdout.print("\n=== ERRORS ({d}) ===\n", .{ast.errors.len});
        for (ast.errors) |err| {
            try stdout.print("  - {s} at token {d}\n", .{ @tagName(err.tag), err.token });
        }
    }

    _ = try std.fs.File.stdout().writeAll(stdout_fbs.getWritten());
}

fn printAst(writer: anytype, ast: mdx.Ast) !void {
    try writer.print("=== AST ===\n", .{});
    try writer.print("Nodes: {d}\n", .{ast.nodes.len});
    try writer.print("Tokens: {d}\n", .{ast.tokens.len});
    try writer.print("Extra data: {d}\n", .{ast.extra_data.len});
    try writer.print("\n=== NODES ===\n", .{});

    // Print all nodes (non-recursive, flat list)
    const node_tags = ast.nodes.items(.tag);
    const node_main_tokens = ast.nodes.items(.main_token);

    for (0..node_tags.len) |i| {
        const node_idx: mdx.Ast.NodeIndex = @intCast(i);
        const tag = node_tags[i];
        const main_token = node_main_tokens[i];
        try writer.print("[{d}] {s}", .{ node_idx, @tagName(tag) });

        // Print node-specific information
        switch (tag) {
            .heading => {
                const info = ast.headingInfo(node_idx);
                try writer.print(" (level={d}, children={d})", .{
                    info.level,
                    info.children_end - info.children_start,
                });
            },
            .text => {
                const token_text = ast.tokenSlice(main_token);
                const trimmed = if (token_text.len > 50) token_text[0..50] else token_text;
                try writer.print(" \"{s}\"", .{trimmed});
            },
            .document, .paragraph, .blockquote, .list_unordered, .list_ordered, .list_item, .mdx_jsx_element, .mdx_jsx_fragment => {
                const children = ast.children(node_idx);
                if (children.len > 0) {
                    try writer.print(" (children={d})", .{children.len});
                }
            },
            else => {},
        }

        try writer.print("\n", .{});
    }

    // Print document tree from root (document is always the last node)
    try writer.print("\n=== TREE ===\n", .{});
    if (ast.nodes.len > 0) {
        const root_idx: mdx.Ast.NodeIndex = @intCast(ast.nodes.len - 1);
        try printNode(writer, ast, root_idx, 0);
    }
}

fn printNode(writer: anytype, ast: mdx.Ast, node_idx: mdx.Ast.NodeIndex, indent: usize) !void {
    const node = ast.nodes.get(node_idx);

    // Print indentation
    for (0..indent) |_| {
        try writer.print("  ", .{});
    }

    // Print node info
    try writer.print("[{d}] {s}", .{ node_idx, @tagName(node.tag) });

    // Print node-specific information
    switch (node.tag) {
        .heading => {
            const info = ast.headingInfo(node_idx);
            try writer.print(" (level={d})", .{info.level});
        },
        .text => {
            const token_text = ast.tokenSlice(node.main_token);
            try writer.print(" \"{s}\"", .{token_text});
        },
        .mdx_text_expression, .mdx_flow_expression => {
            const range = ast.extraData(node.data.extra, mdx.Ast.Node.Range);
            var expr_text: std.ArrayList(u8) = .{};
            defer expr_text.deinit(std.heap.page_allocator);
            for (range.start..range.end) |tok_idx| {
                const tok_idx_u32: mdx.Ast.TokenIndex = @intCast(tok_idx);
                const text = ast.tokenSlice(tok_idx_u32);
                try expr_text.appendSlice(std.heap.page_allocator, text);
            }
            try writer.print(" {{{s}}}", .{expr_text.items});
        },
        .mdx_jsx_element, .mdx_jsx_self_closing => {
            const elem = ast.jsxElement(node_idx);
            const name = ast.tokenSlice(elem.name_token);
            try writer.print(" <{s}>", .{name});
        },
        .link, .image => {
            const link = ast.extraData(node.data.extra, mdx.Ast.Link);
            const url = ast.tokenSlice(link.url_token);
            try writer.print(" (url={s})", .{url});
        },
        .frontmatter => {
            try writer.print(" (YAML frontmatter)", .{});
        },
        else => {},
    }

    try writer.print("\n", .{});

    // Print children
    const children = ast.children(node_idx);
    for (children) |child_idx| {
        try printNode(writer, ast, child_idx, indent + 1);
    }
}
