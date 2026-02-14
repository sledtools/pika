const std = @import("std");
const mdx = @import("lib.zig");

pub const Status = enum(c_int) {
    ok = 0,
    parse_error = 1,
    invalid_utf8 = 2,
    alloc_error = 3,
    internal_error = 4,
};

fn statusCode(s: Status) c_int {
    return @intFromEnum(s);
}

/// Parse markdown/MDX into a JSON AST.
///
/// Caller owns `out_json_ptr` and must free it with `zigmdx_free_json`.
export fn zigmdx_parse_json(
    input_ptr: [*c]const u8,
    input_len: usize,
    out_json_ptr: *[*c]u8,
    out_json_len: *usize,
) c_int {
    out_json_ptr.* = null;
    out_json_len.* = 0;

    if (input_len > 0 and input_ptr == null) {
        return statusCode(.invalid_utf8);
    }

    const input: []const u8 = if (input_len == 0)
        ""
    else
        input_ptr[0..input_len];

    if (!std.unicode.utf8ValidateSlice(input)) {
        return statusCode(.invalid_utf8);
    }

    const allocator = std.heap.c_allocator;

    const source = allocator.allocSentinel(u8, input_len, 0) catch {
        return statusCode(.alloc_error);
    };
    defer allocator.free(source);
    if (input_len > 0) {
        @memcpy(source[0..input_len], input);
    }

    var ast = mdx.parse(allocator, source) catch |err| {
        return switch (err) {
            error.OutOfMemory => statusCode(.alloc_error),
            else => statusCode(.parse_error),
        };
    };
    defer ast.deinit(allocator);

    var json: std.ArrayList(u8) = .{};
    defer json.deinit(allocator);

    mdx.TreeBuilder.serializeTree(&ast, &json, allocator) catch |err| {
        return switch (err) {
            error.OutOfMemory => statusCode(.alloc_error),
        };
    };

    const output = allocator.alloc(u8, json.items.len) catch {
        return statusCode(.alloc_error);
    };
    if (json.items.len > 0) {
        @memcpy(output, json.items);
    }

    out_json_ptr.* = output.ptr;
    out_json_len.* = output.len;
    return statusCode(.ok);
}

/// Free a JSON buffer previously returned by `zigmdx_parse_json`.
export fn zigmdx_free_json(ptr: [*c]u8, len: usize) void {
    if (ptr == null or len == 0) {
        return;
    }
    std.heap.c_allocator.free(ptr[0..len]);
}

/// Increment when ABI changes in an incompatible way.
export fn zigmdx_abi_version() u32 {
    return 1;
}
