const std = @import("std");
const mdx = @import("src/lib.zig");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const tests = [_]struct {
        name: []const u8,
        source: [:0]const u8,
        expect_nodes: usize,
    }{
        .{
            .name = "Heading",
            .source = "# Hello World\n",
            .expect_nodes = 3, // heading, text, document
        },
        .{
            .name = "Strong",
            .source = "**bold**\n",
            .expect_nodes = 4, // paragraph, strong, text, document
        },
        .{
            .name = "Emphasis",
            .source = "*italic*\n",
            .expect_nodes = 4, // paragraph, emphasis, text, document
        },
        .{
            .name = "Mixed inline",
            .source = "Text with **bold** and *italic*\n",
            .expect_nodes = 8, // paragraph, 3x text, strong, text (in strong), emphasis, text (in emphasis), document
        },
        .{
            .name = "Expression",
            .source = "Count: {state.count}\n",
            .expect_nodes = 4, // paragraph, text, expression, document
        },
        .{
            .name = "JSX self-closing",
            .source = "<button label=\"Click\" />\n",
            .expect_nodes = 3, // jsx_self_closing, paragraph, document
        },
    };

    const stdout = std.io.getStdOut().writer();
    var passed: usize = 0;
    var failed: usize = 0;

    for (tests) |t| {
        var ast = mdx.parse(allocator, t.source) catch |err| {
            try stdout.print("❌ {s}: Parse failed with {}\n", .{ t.name, err });
            failed += 1;
            continue;
        };
        defer ast.deinit(allocator);

        const success = ast.nodes.len == t.expect_nodes and ast.errors.len == 0;
        const icon = if (success) "✅" else "❌";

        try stdout.print("{s} {s}: {d} nodes (expected {d}), {d} errors\n", .{
            icon,
            t.name,
            ast.nodes.len,
            t.expect_nodes,
            ast.errors.len,
        });

        if (success) {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    try stdout.print("\n{d} passed, {d} failed\n", .{ passed, failed });
    if (failed > 0) {
        std.process.exit(1);
    }
}
