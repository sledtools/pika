const std = @import("std");
const mdx = @import("src/lib.zig");

test "paragraph behavior: single line break" {
    const source = "Line one\nLine two\n";
    var ast = try mdx.parse(std.testing.allocator, source);
    defer ast.deinit(std.testing.allocator);

    // Count paragraphs - single break should be ONE paragraph
    var para_count: u32 = 0;
    for (0..ast.nodes.len) |i| {
        const node = ast.nodes.get(@intCast(i));
        if (node.tag == .paragraph) {
            para_count += 1;
        }
    }
    
    std.debug.print("\nSingle line break test:\n", .{});
    std.debug.print("  Source: {s}\n", .{ source });
    std.debug.print("  Total nodes: {d}\n", .{ ast.nodes.len });
    std.debug.print("  Paragraph count: {d}\n", .{ para_count });
    std.debug.print("  Expected: 1 paragraph (but may be getting more due to bug)\n", .{});
}

test "paragraph behavior: double line break" {
    const source = "Paragraph one\n\nParagraph two\n";
    var ast = try mdx.parse(std.testing.allocator, source);
    defer ast.deinit(std.testing.allocator);

    // Count paragraphs - double break should be TWO paragraphs
    var para_count: u32 = 0;
    for (0..ast.nodes.len) |i| {
        const node = ast.nodes.get(@intCast(i));
        if (node.tag == .paragraph) {
            para_count += 1;
        }
    }
    
    std.debug.print("\nDouble line break test:\n", .{});
    std.debug.print("  Source: {s}\n", .{ source });
    std.debug.print("  Total nodes: {d}\n", .{ ast.nodes.len });
    std.debug.print("  Paragraph count: {d}\n", .{ para_count });
    std.debug.print("  Expected: 2 paragraphs\n", .{});
}

test "paragraph behavior: multiple single breaks" {
    const source = "Line one\nLine two\nLine three\n";
    var ast = try mdx.parse(std.testing.allocator, source);
    defer ast.deinit(std.testing.allocator);

    // Count paragraphs - multiple single breaks should be ONE paragraph
    var para_count: u32 = 0;
    for (0..ast.nodes.len) |i| {
        const node = ast.nodes.get(@intCast(i));
        if (node.tag == .paragraph) {
            para_count += 1;
        }
    }
    
    std.debug.print("\nMultiple single breaks test:\n", .{});
    std.debug.print("  Source: {s}\n", .{ source });
    std.debug.print("  Total nodes: {d}\n", .{ ast.nodes.len });
    std.debug.print("  Paragraph count: {d}\n", .{ para_count });
    std.debug.print("  Expected: 1 paragraph (but may be getting more due to bug)\n", .{});
}
