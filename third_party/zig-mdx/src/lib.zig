pub const Token = @import("Token.zig");
pub const Tokenizer = @import("Tokenizer.zig");
pub const Ast = @import("Ast.zig");
pub const Parser = @import("Parser.zig");
pub const TreeBuilder = @import("TreeBuilder.zig");
pub const Render = @import("Render.zig");

/// Parse MDX source into an AST
pub const parse = Parser.parse;

/// Render AST back to MDX source
pub const render = Render.render;
pub const renderAlloc = Render.renderAlloc;

/// Cursor mapping utilities
pub const nodeSpan = Ast.nodeSpan;
pub const nodeAtOffset = Ast.nodeAtOffset;

test {
    @import("std").testing.refAllDecls(@This());
}
