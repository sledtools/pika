const std = @import("std");
const Tokenizer = @import("src/Tokenizer.zig");
const Token = @import("src/Token.zig");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // Test basic markdown
    const tests = [_]struct {
        name: []const u8,
        source: [:0]const u8,
    }{
        .{ .name = "Heading", .source = "# Hello\n" },
        .{ .name = "Strong", .source = "**bold**" },
        .{ .name = "Emphasis", .source = "*italic*" },
        .{ .name = "Expression", .source = "{state.count}" },
        .{ .name = "JSX", .source = "<button />" },
        .{ .name = "Frontmatter", .source = "---\ntest: 1\n---\n" },
    };

    const stdout = std.io.getStdOut().writer();

    for (tests) |t| {
        try stdout.print("\n=== Test: {s} ===\n", .{t.name});
        try stdout.print("Source: \"{s}\"\n", .{t.source});

        var tokenizer = Tokenizer.init(t.source, allocator);
        defer tokenizer.deinit();

        var count: u32 = 0;
        while (true) {
            const tok = tokenizer.next();
            const text = t.source[tok.loc.start..tok.loc.end];
            try stdout.print("  [{d}] {s}: \"{s}\"\n", .{ count, @tagName(tok.tag), text });

            count += 1;
            if (tok.tag == .eof) break;
            if (count > 20) {
                try stdout.print("  ... (stopping at 20 tokens)\n", .{});
                break;
            }
        }
    }
}
