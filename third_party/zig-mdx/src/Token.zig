const std = @import("std");

/// Token represents a single lexical unit in MDX source.
/// Tokens track their position but not their text content -
/// use Loc indices into the source buffer to retrieve text.
tag: Tag,
loc: Loc,

const Token = @This();

pub const Loc = struct {
    start: u32,
    end: u32,
};

pub const Tag = enum {
    // Markdown block-level tokens
    heading_start, // #, ##, ###, etc. (level determined by counting #)
    paragraph_start,
    code_fence_start, // ```
    code_fence_end,
    list_item_unordered, // -, *, +
    list_item_ordered, // 1., 2., etc.
    blockquote_start, // >
    hr, // --- or *** or ___
    blank_line, // Empty line (significant for MDX block/inline distinction)

    // Markdown inline tokens
    text, // Plain text content
    strong_start, // **
    strong_end, // **
    emphasis_start, // *
    emphasis_end, // *
    code_inline_start, // `
    code_inline_end, // `
    link_start, // [
    link_end, // ]
    link_url_start, // (
    link_url_end, // )
    image_start, // ![
    hard_break, // Two trailing spaces + \n or backslash + \n

    // MDX Expression tokens
    expr_start, // {
    expr_end, // }

    // JSX tokens
    jsx_tag_start, // <
    jsx_tag_end, // >
    jsx_close_tag, // </
    jsx_self_close, // />
    jsx_fragment_start, // <>
    jsx_fragment_close, // </>
    jsx_identifier, // Component name or attribute name
    jsx_dot, // . for member expressions
    jsx_colon, // : for namespaced attributes
    jsx_equal, // = for attribute values
    jsx_string, // "value" or 'value'
    jsx_attr_expr_start, // {expr}

    // Frontmatter tokens
    frontmatter_start, // --- at start of file
    frontmatter_end, // --- after YAML content
    frontmatter_content, // YAML content between delimiters

    // ESM tokens
    esm_import, // import statement
    esm_export, // export statement

    // Whitespace and structural
    newline, // \n (significant in MDX)
    space, // Spaces (may be significant in some contexts)
    indent, // Leading indentation (tracked for markdown structure)

    // Special
    eof,
    invalid,

    pub fn symbol(tag: Tag) []const u8 {
        return switch (tag) {
            .heading_start => "#",
            .strong_start, .strong_end => "**",
            .emphasis_start, .emphasis_end => "*",
            .code_inline_start, .code_inline_end => "`",
            .link_start => "[",
            .link_end => "]",
            .link_url_start => "(",
            .link_url_end => ")",
            .image_start => "![",
            .expr_start => "{",
            .expr_end => "}",
            .jsx_tag_start => "<",
            .jsx_tag_end => ">",
            .jsx_close_tag => "</",
            .jsx_self_close => "/>",
            .jsx_fragment_start => "<>",
            .jsx_fragment_close => "</>",
            .jsx_dot => ".",
            .jsx_colon => ":",
            .jsx_equal => "=",
            .hr => "---",
            .frontmatter_start, .frontmatter_end => "---",
            .newline => "\\n",
            .eof => "EOF",
            else => @tagName(tag),
        };
    }
};

/// Keywords for ESM statements
pub const keywords = std.StaticStringMap(Tag).initComptime(.{
    .{ "import", .esm_import },
    .{ "export", .esm_export },
});

test "Token.Tag.symbol" {
    try std.testing.expectEqualStrings("#", Tag.heading_start.symbol());
    try std.testing.expectEqualStrings("**", Tag.strong_start.symbol());
    try std.testing.expectEqualStrings("{", Tag.expr_start.symbol());
}
