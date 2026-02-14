const std = @import("std");
const mdx = @import("src/lib.zig");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const source =
        \\# Test Code Blocks
        \\
        \\```typescript
        \\const greeting = "Hello";
        \\console.log(greeting);
        \\```
        \\
        \\Some text after.
    ;

    const stdout = std.io.getStdOut().writer();
    try stdout.print("Parsing code block test...\n\n", .{});
    try stdout.print("Source:\n{s}\n\n", .{source});

    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const node_tags = ast.nodes.items(.tag);

    try stdout.print("=== Nodes ({d}) ===\n", .{node_tags.len});
    for (node_tags, 0..) |tag, i| {
        try stdout.print("[{d}] {s}\n", .{ i, @tagName(tag) });
    }

    try stdout.print("\n=== Tokens ({d}) ===\n", .{ast.tokens.len});
    const token_tags = ast.tokens.items(.tag);
    for (0..@min(20, ast.tokens.len)) |i| {
        const tag = token_tags[i];
        try stdout.print("[{d}] {s}\n", .{ i, @tagName(tag) });
    }

    try stdout.print("\n=== Errors ({d}) ===\n", .{ast.errors.len});
    for (ast.errors) |err| {
        try stdout.print("  - {s} at token {d}\n", .{ @tagName(err.tag), err.token });
    }

    // Check document children
    if (node_tags.len > 0) {
        const doc_idx: mdx.Ast.NodeIndex = @intCast(node_tags.len - 1);
        const children = ast.children(doc_idx);
        try stdout.print("\n✅ Document has {d} children\n", .{children.len});
    }

    // Find code block nodes
    var code_block_count: usize = 0;
    for (node_tags) |tag| {
        if (tag == .code_block) code_block_count += 1;
    }
    try stdout.print("✅ Found {d} code block nodes\n", .{code_block_count});
}
