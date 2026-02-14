const std = @import("std");
const mdx = @import("src/lib.zig");
const TreeBuilder = mdx.TreeBuilder;

test "TreeBuilder serializes simple text" {
    const allocator = std.testing.allocator;

    const source = "Hello world";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(json_str.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"type\":\"root\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "Hello world") != null);
}

test "TreeBuilder serializes heading with level" {
    const allocator = std.testing.allocator;

    const source = "# Hello";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"type\":\"heading\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"level\":1") != null);
}

test "TreeBuilder serializes code block with language" {
    const allocator = std.testing.allocator;

    const source =
        \\```javascript
        \\console.log("hi");
        \\```
    ;
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"type\":\"code_block\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"lang\":\"javascript\"") != null);
}

test "TreeBuilder serializes JSX element with attributes" {
    const allocator = std.testing.allocator;

    const source = "<Button color=\"blue\" />";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"type\":\"mdx_jsx_self_closing\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"name\":\"Button\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"attributes\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "blue") != null);
}

test "TreeBuilder escapes JSON strings" {
    const allocator = std.testing.allocator;

    const source = "Text with \"quotes\" and \\backslash";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\\\"quotes\\\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\\\\backslash") != null);
}

test "TreeBuilder includes errors in output" {
    const allocator = std.testing.allocator;

    const source = "<Unclosed";
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;
    try std.testing.expect(std.mem.indexOf(u8, json_str, "\"errors\"") != null);
}

test "TreeBuilder produces valid JSON" {
    const allocator = std.testing.allocator;

    const source =
        \\# Title
        \\
        \\A paragraph with **bold** text.
        \\
        \\- Item 1
        \\- Item 2
    ;
    var ast = try mdx.parse(allocator, source);
    defer ast.deinit(allocator);

    var output: std.ArrayList(u8) = .{};
    defer output.deinit(allocator);

    try TreeBuilder.serializeTree(&ast, &output, allocator);

    const json_str = output.items;

    // Try to parse it as JSON to validate structure
    const parsed = try std.json.parseFromSlice(std.json.Value, allocator, json_str, .{});
    defer parsed.deinit();

    const root = parsed.value.object;
    try std.testing.expect(root.contains("type"));
    try std.testing.expect(root.contains("children"));
    try std.testing.expect(root.contains("source"));
    try std.testing.expect(root.contains("errors"));
}
