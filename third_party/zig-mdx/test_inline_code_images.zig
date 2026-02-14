const std = @import("std");
const mdx = @import("src/lib.zig");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const source =
        \\# Test Inline Code and Images
        \\
        \\This is `inline code` in a sentence.
        \\
        \\More text with `another code` example.
        \\
        \\![Alt text](image.jpg)
        \\
        \\[Link text](url.com)
    ;

    const stdout = std.io.getStdOut().writer();
    try stdout.print("Parsing inline code and images test...\n\n", .{});

    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    const node_tags = ast.nodes.items(.tag);

    try stdout.print("=== Nodes ({d}) ===\n", .{node_tags.len});
    for (node_tags, 0..) |tag, i| {
        try stdout.print("[{d}] {s}\n", .{ i, @tagName(tag) });
    }

    try stdout.print("\n=== Errors ({d}) ===\n", .{ast.errors.len});
    for (ast.errors) |err| {
        try stdout.print("  - {s} at token {d}\n", .{ @tagName(err.tag), err.token });
    }

    // Find inline code nodes
    var code_count: usize = 0;
    for (node_tags) |tag| {
        if (tag == .code_inline) code_count += 1;
    }
    try stdout.print("\n✅ Found {d} inline code nodes\n", .{code_count});

    // Find image nodes
    var image_count: usize = 0;
    for (node_tags) |tag| {
        if (tag == .image) image_count += 1;
    }
    try stdout.print("✅ Found {d} image nodes\n", .{image_count});

    // Check image textNode
    for (node_tags, 0..) |tag, i| {
        if (tag == .image) {
            const node_idx: mdx.Ast.NodeIndex = @intCast(i);
            const node = ast.nodes.get(node_idx);
            const link = ast.extraData(node.data.extra, mdx.Ast.Link);
            try stdout.print("✅ Image node [{d}] textNode: {d}\n", .{ i, link.text_node });
            if (link.text_node > 0 and link.text_node != i) {
                try stdout.print("   ✓ textNode points to separate node (not self)\n", .{});
            } else {
                try stdout.print("   ✗ ERROR: textNode points to self!\n", .{});
            }
        }
    }
}
