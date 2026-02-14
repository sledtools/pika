const std = @import("std");
const mdx = @import("lib.zig");

// Fixed buffer allocator for WASM, wrapped in arena for proper cleanup
var buffer: [8 * 1024 * 1024]u8 = undefined; // 8MB buffer
var fba: ?std.heap.FixedBufferAllocator = null;
var arena: ?std.heap.ArenaAllocator = null;

fn getAllocator() std.mem.Allocator {
    if (arena == null) {
        fba = std.heap.FixedBufferAllocator.init(&buffer);
        arena = std.heap.ArenaAllocator.init(fba.?.allocator());
    }
    return arena.?.allocator();
}

/// Initialize WASM module
export fn wasm_init() void {
    fba = std.heap.FixedBufferAllocator.init(&buffer);
    arena = std.heap.ArenaAllocator.init(fba.?.allocator());
}

/// Get library version
export fn wasm_get_version() u32 {
    return 1;
}

/// Allocate memory
export fn wasm_alloc(size: usize) ?[*]u8 {
    const mem = getAllocator().alloc(u8, size) catch return null;
    return mem.ptr;
}

/// Free memory
export fn wasm_free(ptr: [*]u8, size: usize) void {
    getAllocator().free(ptr[0..size]);
}

/// Parse MDX source and return JSON AST
/// Returns true on success, false on error
export fn wasm_parse_mdx(
    source_ptr: [*]const u8,
    source_len: u32,
    out_json_ptr: *[*]u8,
    out_json_len: *u32,
) bool {
    const allocator = getAllocator();

    // Allocate sentinel-terminated string
    const source_sentinel = allocator.allocSentinel(u8, source_len, 0) catch return false;
    defer allocator.free(source_sentinel);
    @memcpy(source_sentinel, source_ptr[0..source_len]);

    // Parse the MDX
    var ast = mdx.parse(allocator, source_sentinel) catch return false;
    defer ast.deinit(allocator);

    // Serialize AST to JSON tree structure
    var json_string: std.ArrayList(u8) = .{};
    defer json_string.deinit(allocator);

    mdx.TreeBuilder.serializeTree(&ast, &json_string, allocator) catch return false;

    // Allocate output buffer
    const output = allocator.alloc(u8, json_string.items.len) catch return false;
    @memcpy(output, json_string.items);

    out_json_ptr.* = output.ptr;
    out_json_len.* = @intCast(output.len);

    return true;
}

/// Reset the allocator (useful for freeing all memory at once)
export fn wasm_reset() void {
    if (arena) |*a| {
        _ = a.reset(.retain_capacity);
    }
}

/// Parse MDX source and return JSON AST with position info for each node
/// Useful for editor integrations that need cursor-to-node mapping
export fn wasm_parse_mdx_with_positions(
    source_ptr: [*]const u8,
    source_len: u32,
    out_json_ptr: *[*]u8,
    out_json_len: *u32,
) bool {
    const allocator = getAllocator();

    // Allocate sentinel-terminated string
    const source_sentinel = allocator.allocSentinel(u8, source_len, 0) catch return false;
    defer allocator.free(source_sentinel);
    @memcpy(source_sentinel, source_ptr[0..source_len]);

    // Parse the MDX
    var ast = mdx.parse(allocator, source_sentinel) catch return false;
    defer ast.deinit(allocator);

    // Serialize AST to JSON tree structure with positions
    var json_string: std.ArrayList(u8) = .{};
    defer json_string.deinit(allocator);

    mdx.TreeBuilder.serializeTreeWithOptions(&ast, &json_string, allocator, .{
        .include_positions = true,
    }) catch return false;

    // Allocate output buffer
    const output = allocator.alloc(u8, json_string.items.len) catch return false;
    @memcpy(output, json_string.items);

    out_json_ptr.* = output.ptr;
    out_json_len.* = @intCast(output.len);

    return true;
}

/// Render AST back to MDX source (for roundtripping)
/// Takes JSON AST input, returns MDX string
/// Note: This parses the source, then renders - JSON AST input is for API consistency
export fn wasm_render_mdx(
    source_ptr: [*]const u8,
    source_len: u32,
    out_mdx_ptr: *[*]u8,
    out_mdx_len: *u32,
) bool {
    const allocator = getAllocator();

    // Allocate sentinel-terminated string
    const source_sentinel = allocator.allocSentinel(u8, source_len, 0) catch return false;
    defer allocator.free(source_sentinel);
    @memcpy(source_sentinel, source_ptr[0..source_len]);

    // Parse the MDX
    var ast = mdx.parse(allocator, source_sentinel) catch return false;
    defer ast.deinit(allocator);

    // Render AST back to MDX
    var mdx_string: std.ArrayList(u8) = .{};
    defer mdx_string.deinit(allocator);

    mdx.Render.render(&ast, &mdx_string, allocator) catch return false;

    // Allocate output buffer
    const output = allocator.alloc(u8, mdx_string.items.len) catch return false;
    @memcpy(output, mdx_string.items);

    out_mdx_ptr.* = output.ptr;
    out_mdx_len.* = @intCast(output.len);

    return true;
}
