const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // Executable
    const exe_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    const exe = b.addExecutable(.{
        .name = "mdx-parse",
        .root_module = exe_module,
    });
    b.installArtifact(exe);

    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());
    if (b.args) |args| {
        run_cmd.addArgs(args);
    }

    const run_step = b.step("run", "Run the MDX parser");
    run_step.dependOn(&run_cmd.step);

    // Tests
    const test_module = b.createModule(.{
        .root_source_file = b.path("src/lib.zig"),
        .target = target,
        .optimize = optimize,
    });
    const lib_unit_tests = b.addTest(.{
        .root_module = test_module,
    });

    const run_lib_unit_tests = b.addRunArtifact(lib_unit_tests);

    // TreeBuilder tests
    const tree_builder_test_module = b.createModule(.{
        .root_source_file = b.path("test_tree_builder.zig"),
        .target = target,
        .optimize = optimize,
    });
    const tree_builder_tests = b.addTest(.{
        .root_module = tree_builder_test_module,
    });

    const run_tree_builder_tests = b.addRunArtifact(tree_builder_tests);

    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_lib_unit_tests.step);
    test_step.dependOn(&run_tree_builder_tests.step);

    // Native static library for FFI (C ABI)
    const static_module = b.createModule(.{
        .root_source_file = b.path("src/c_api.zig"),
        .target = target,
        .optimize = optimize,
    });
    const static_lib = b.addLibrary(.{
        .name = "zigmdx",
        .linkage = .static,
        .root_module = static_module,
    });
    static_lib.linkLibC();

    const install_static = b.addInstallArtifact(static_lib, .{});
    const install_header = b.addInstallFile(b.path("src/zigmdx.h"), "include/zigmdx.h");

    const static_step = b.step("static", "Build static C ABI library");
    static_step.dependOn(&install_static.step);
    static_step.dependOn(&install_header.step);

    // WASM build
    const wasm_target = b.resolveTargetQuery(.{
        .cpu_arch = .wasm32,
        .os_tag = .freestanding,
    });

    const wasm_module = b.createModule(.{
        .root_source_file = b.path("src/wasm_exports.zig"),
        .target = wasm_target,
        .optimize = .ReleaseSmall,
    });
    const wasm_lib = b.addExecutable(.{
        .name = "zigmdx",
        .root_module = wasm_module,
    });

    wasm_lib.rdynamic = true;
    wasm_lib.entry = .disabled;
    wasm_lib.export_memory = true;

    // Set memory limits
    wasm_lib.stack_size = 1024 * 1024; // 1MB stack
    wasm_lib.initial_memory = 16 * 1024 * 1024; // 16MB initial
    wasm_lib.max_memory = 32 * 1024 * 1024; // 32MB max

    b.installArtifact(wasm_lib);

    // Install WASM file to wasm package directory
    const wasm_install_file = b.addInstallFile(
        wasm_lib.getEmittedBin(),
        "wasm/src/mdx.wasm",
    );

    // Create wasm build step
    const wasm_step = b.step("wasm", "Build WASM library");
    wasm_step.dependOn(&wasm_lib.step);
    wasm_step.dependOn(&wasm_install_file.step);
}
