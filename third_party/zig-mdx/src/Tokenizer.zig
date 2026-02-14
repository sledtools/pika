const std = @import("std");
const Token = @import("Token.zig");
const Allocator = std.mem.Allocator;

allocator: Allocator,
buffer: [:0]const u8,
index: u32,
line_start: u32,
mode: Mode,
mode_stack: std.ArrayList(Mode),
strong_depth: u32,
emphasis_depth: u32,
after_link_text: bool, // True after ] that could be end of link text
in_link_url: bool, // True when inside (url) part of a link

const Tokenizer = @This();

pub const Mode = enum {
    markdown,
    jsx,
    expression,
    inline_code,
    code_block,
};

const State = enum {
    start,
    start_of_line,
    heading,
    text,
    maybe_strong_or_emphasis,
    strong,
    emphasis,
    code_fence_start,
    code_fence_lang,
    hr_or_frontmatter,
    newline,
    jsx_tag_name,
    jsx_attributes,
    jsx_attr_value,
    jsx_string,
    expression,
};

pub fn init(buffer: [:0]const u8, allocator: Allocator) Tokenizer {
    return .{
        .allocator = allocator,
        .buffer = buffer,
        .index = 0,
        .line_start = 0,
        .mode = .markdown,
        .mode_stack = .{},
        .strong_depth = 0,
        .emphasis_depth = 0,
        .after_link_text = false,
        .in_link_url = false,
    };
}

pub fn deinit(self: *Tokenizer) void {
    self.mode_stack.deinit(self.allocator);
}

pub fn next(self: *Tokenizer) Token {
    return switch (self.mode) {
        .markdown => self.nextMarkdown(),
        .jsx => self.nextJsx(),
        .expression => self.nextExpression(),
        .inline_code => self.nextInlineCode(),
        .code_block => self.nextCodeBlock(),
    };
}

fn nextMarkdown(self: *Tokenizer) Token {
    const start = self.index;

    // Handle EOF
    if (self.index >= self.buffer.len) {
        return .{ .tag = .eof, .loc = .{ .start = @intCast(self.index), .end = @intCast(self.index) } };
    }

    var state: State = if (self.index == self.line_start) .start_of_line else .start;

    while (true) {
        const c = self.buffer[self.index];

        switch (state) {
            .start_of_line => {
                switch (c) {
                    0 => return self.makeToken(.eof, start),
                    '\n' => {
                        self.index += 1;
                        self.line_start = self.index;
                        return self.makeToken(.blank_line, start);
                    },
                    '#' => {
                        state = .heading;
                        self.index += 1;
                    },
                    '-', '*', '_' => {
                        state = .hr_or_frontmatter;
                        self.index += 1;
                    },
                    '`' => {
                        if (self.peekAhead("```")) {
                            self.index += 3;
                            self.pushMode(.code_block) catch {
                                return self.makeToken(.invalid, start);
                            };
                            return self.makeToken(.code_fence_start, start);
                        }
                        state = .text;
                    },
                    '>' => {
                        self.index += 1;
                        return self.makeToken(.blockquote_start, start);
                    },
                    ' ', '\t' => {
                        // Track indentation
                        const indent_start = self.index;
                        while (self.buffer[self.index] == ' ' or self.buffer[self.index] == '\t') {
                            self.index += 1;
                        }
                        return self.makeToken(.indent, indent_start);
                    },
                    '0'...'9' => {
                        // Check for ordered list (e.g., "1. ")
                        var temp_index = self.index;
                        while (temp_index < self.buffer.len and self.buffer[temp_index] >= '0' and self.buffer[temp_index] <= '9') {
                            temp_index += 1;
                        }
                        if (temp_index < self.buffer.len and self.buffer[temp_index] == '.' and
                            temp_index + 1 < self.buffer.len and self.buffer[temp_index + 1] == ' ')
                        {
                            self.index = temp_index + 2; // Skip digits, dot, and space
                            return self.makeToken(.list_item_ordered, start);
                        }
                        state = .start;
                    },
                    else => {
                        state = .start;
                    },
                }
            },

            .start => {
                switch (c) {
                    0 => return self.makeToken(.eof, start),
                    '\n' => {
                        self.index += 1;
                        self.line_start = self.index;
                        return self.makeToken(.newline, start);
                    },
                    '\\' => {
                        // Check if this is a hard break (backslash followed by newline)
                        if (self.index + 1 < self.buffer.len and self.buffer[self.index + 1] == '\n') {
                            self.index += 2; // Skip backslash and newline
                            self.line_start = self.index;
                            return self.makeToken(.hard_break, start);
                        }
                        // Otherwise treat as text
                        state = .text;
                    },
                    ' ' => {
                        // Check if this is a hard break (two+ spaces followed by newline)
                        var space_count: u32 = 0;
                        var temp_idx = self.index;
                        while (temp_idx < self.buffer.len and self.buffer[temp_idx] == ' ') {
                            space_count += 1;
                            temp_idx += 1;
                        }
                        if (space_count >= 2 and temp_idx < self.buffer.len and self.buffer[temp_idx] == '\n') {
                            self.index = temp_idx + 1; // Skip spaces and newline
                            self.line_start = self.index;
                            return self.makeToken(.hard_break, start);
                        }
                        // Otherwise treat as text
                        state = .text;
                    },
                    '{' => {
                        self.index += 1;
                        self.pushMode(.expression) catch {
                            return self.makeToken(.invalid, start);
                        };
                        return self.makeToken(.expr_start, start);
                    },
                    '<' => {
                        // Check if this is JSX or autolink
                        if (self.isJsxStart()) {
                            self.pushMode(.jsx) catch {
                                return self.makeToken(.invalid, start);
                            };
                            return self.nextJsx();
                        }
                        // Otherwise treat as text
                        state = .text;
                    },
                    '*' => {
                        state = .maybe_strong_or_emphasis;
                        self.index += 1;
                    },
                    '`' => {
                        self.index += 1;
                        self.pushMode(.inline_code) catch {
                            return self.makeToken(.invalid, start);
                        };
                        return self.makeToken(.code_inline_start, start);
                    },
                    '[' => {
                        self.index += 1;
                        self.after_link_text = false;
                        return self.makeToken(.link_start, start);
                    },
                    ']' => {
                        self.index += 1;
                        // Check if this is followed by ( for link URL
                        if (self.buffer[self.index] == '(') {
                            self.after_link_text = true;
                            return self.makeToken(.link_end, start);
                        }
                        // Otherwise treat ] as text
                        self.after_link_text = false;
                        state = .text;
                    },
                    '(' => {
                        if (self.after_link_text) {
                            self.index += 1;
                            self.after_link_text = false;
                            self.in_link_url = true;
                            return self.makeToken(.link_url_start, start);
                        }
                        // Not in link context, treat as text
                        state = .text;
                    },
                    ')' => {
                        if (self.in_link_url) {
                            self.index += 1;
                            self.in_link_url = false;
                            return self.makeToken(.link_url_end, start);
                        }
                        // Not in link URL context, treat as text
                        state = .text;
                    },
                    '!' => {
                        if (self.index + 1 < self.buffer.len and self.buffer[self.index + 1] == '[') {
                            self.index += 2;
                            return self.makeToken(.image_start, start);
                        }
                        // Just a literal '!' - treat as text
                        self.index += 1;
                        state = .text;
                    },
                    else => {
                        state = .text;
                    },
                }
            },

            .heading => {
                // Count consecutive # characters
                while (self.buffer[self.index] == '#') {
                    self.index += 1;
                }
                // Skip space after #
                if (self.buffer[self.index] == ' ') {
                    self.index += 1;
                }
                return self.makeToken(.heading_start, start);
            },

            .hr_or_frontmatter => {
                const first_char = self.buffer[start];
                var count: u32 = 1;

                while (self.buffer[self.index] == first_char) {
                    count += 1;
                    self.index += 1;
                }

                // Check for frontmatter (--- at start of file)
                if (first_char == '-' and count >= 3 and start == 0) {
                    if (self.buffer[self.index] == '\n' or self.buffer[self.index] == 0) {
                        return self.makeToken(.frontmatter_start, start);
                    }
                }

                // Check for HR (3+ consecutive -, *, or _)
                if (count >= 3 and (self.buffer[self.index] == '\n' or self.buffer[self.index] == 0)) {
                    return self.makeToken(.hr, start);
                }

                // Check for list item
                if (first_char == '-' or first_char == '*') {
                    if (self.buffer[self.index] == ' ') {
                        return self.makeToken(.list_item_unordered, start);
                    }
                }

                // Special case: * or ** at line start could be emphasis/strong
                if (first_char == '*') {
                    // Reset index to start and handle as emphasis/strong
                    self.index = start + 1; // Move past first *
                    state = .maybe_strong_or_emphasis;
                } else {
                    // Otherwise, treat as text
                    state = .text;
                }
            },

            .maybe_strong_or_emphasis => {
                if (self.buffer[self.index] == '*') {
                    self.index += 1;
                    // Check if we're closing or opening strong
                    if (self.strong_depth > 0) {
                        self.strong_depth -= 1;
                        return self.makeToken(.strong_end, start);
                    } else {
                        self.strong_depth += 1;
                        return self.makeToken(.strong_start, start);
                    }
                } else {
                    // Check if we're closing or opening emphasis
                    if (self.emphasis_depth > 0) {
                        self.emphasis_depth -= 1;
                        return self.makeToken(.emphasis_end, start);
                    } else {
                        self.emphasis_depth += 1;
                        return self.makeToken(.emphasis_start, start);
                    }
                }
            },

            .text => {
                // Consume text until we hit a special character
                while (self.index < self.buffer.len) {
                    const ch = self.buffer[self.index];
                    switch (ch) {
                        0, '\n', '{', '<', '*', '`', '[' => break,
                        ']' => {
                            // Only break if followed by ( (potential link)
                            if (self.index + 1 < self.buffer.len and self.buffer[self.index + 1] == '(') {
                                break;
                            }
                            self.index += 1;
                        },
                        '(' => {
                            // Only break if we're expecting a link URL
                            if (self.after_link_text) break;
                            self.index += 1;
                        },
                        ')' => {
                            // Only break if we're in a link URL
                            if (self.in_link_url) break;
                            self.index += 1;
                        },
                        '!' => {
                            // Only break if followed by [ (potential image)
                            if (self.index + 1 < self.buffer.len and self.buffer[self.index + 1] == '[') {
                                break;
                            }
                            self.index += 1;
                        },
                        else => self.index += 1,
                    }
                }

                // Check if we have a hard break pattern at the end
                // (trailing spaces or backslash before newline)
                if (self.index < self.buffer.len and self.buffer[self.index] == '\n') {
                    // Check for backslash immediately before newline
                    if (self.index > start and self.buffer[self.index - 1] == '\\') {
                        self.index -= 1; // Don't include the backslash in text
                        // If this left us with an empty text token, skip to hard_break
                        if (self.index == start) {
                            self.index += 2; // Skip backslash and newline
                            self.line_start = self.index;
                            return self.makeToken(.hard_break, start);
                        }
                    } else {
                        // Check for two+ trailing spaces
                        var end_idx = self.index;
                        var spaces: u32 = 0;
                        while (end_idx > start and self.buffer[end_idx - 1] == ' ') {
                            spaces += 1;
                            end_idx -= 1;
                        }
                        if (spaces >= 2) {
                            // If text would be empty, emit hard_break directly
                            if (end_idx == start) {
                                self.index += 1; // Skip the newline
                                self.line_start = self.index;
                                return self.makeToken(.hard_break, start);
                            }
                            self.index = end_idx; // Don't include trailing spaces in text
                        }
                    }
                }

                return self.makeToken(.text, start);
            },

            else => {
                self.index += 1;
                return self.makeToken(.invalid, start);
            },
        }
    }
}

fn nextJsx(self: *Tokenizer) Token {
    const start = self.index;

    if (self.index >= self.buffer.len) {
        return .{ .tag = .eof, .loc = .{ .start = @intCast(self.index), .end = @intCast(self.index) } };
    }

    const c = self.buffer[self.index];

    switch (c) {
        0 => return self.makeToken(.eof, start),
        '<' => {
            self.index += 1;
            // Check for closing tag
            if (self.buffer[self.index] == '/') {
                self.index += 1;
                return self.makeToken(.jsx_close_tag, start);
            }
            // Check for fragment
            if (self.buffer[self.index] == '>') {
                self.index += 1;
                return self.makeToken(.jsx_fragment_start, start);
            }
            return self.makeToken(.jsx_tag_start, start);
        },
        '>' => {
            self.index += 1;
            self.popMode();
            return self.makeToken(.jsx_tag_end, start);
        },
        '/' => {
            if (self.buffer[self.index + 1] == '>') {
                self.index += 2;
                self.popMode();
                return self.makeToken(.jsx_self_close, start);
            }
            self.index += 1;
            return self.makeToken(.invalid, start);
        },
        '{' => {
            self.index += 1;
            self.pushMode(.expression) catch {
                return self.makeToken(.invalid, start);
            };
            return self.makeToken(.jsx_attr_expr_start, start);
        },
        '=' => {
            self.index += 1;
            return self.makeToken(.jsx_equal, start);
        },
        '"', '\'' => {
            return self.nextJsxString(c);
        },
        '.' => {
            self.index += 1;
            return self.makeToken(.jsx_dot, start);
        },
        ':' => {
            self.index += 1;
            return self.makeToken(.jsx_colon, start);
        },
        ' ', '\t', '\n' => {
            // Skip whitespace
            while (self.index < self.buffer.len) {
                const ch = self.buffer[self.index];
                if (ch != ' ' and ch != '\t' and ch != '\n') break;
                self.index += 1;
            }
            return self.next(); // Get next real token
        },
        'a'...'z', 'A'...'Z', '_' => {
            return self.nextJsxIdentifier();
        },
        else => {
            self.index += 1;
            return self.makeToken(.invalid, start);
        },
    }
}

fn nextJsxIdentifier(self: *Tokenizer) Token {
    const start = self.index;

    while (self.index < self.buffer.len) {
        const c = self.buffer[self.index];
        switch (c) {
            'a'...'z', 'A'...'Z', '0'...'9', '_', '-' => self.index += 1,
            else => break,
        }
    }

    return self.makeToken(.jsx_identifier, start);
}

fn nextJsxString(self: *Tokenizer, quote: u8) Token {
    const start = self.index;
    self.index += 1; // Skip opening quote

    while (self.index < self.buffer.len) {
        const c = self.buffer[self.index];
        if (c == quote) {
            self.index += 1;
            return self.makeToken(.jsx_string, start);
        }
        if (c == '\\') {
            self.index += 2; // Skip escape sequence
        } else {
            self.index += 1;
        }
    }

    return self.makeToken(.invalid, start);
}

fn nextExpression(self: *Tokenizer) Token {
    const start = self.index;

    if (self.index >= self.buffer.len) {
        return .{ .tag = .eof, .loc = .{ .start = @intCast(self.index), .end = @intCast(self.index) } };
    }

    const c = self.buffer[self.index];

    switch (c) {
        0 => return self.makeToken(.eof, start),
        '}' => {
            self.index += 1;
            self.popMode();
            return self.makeToken(.expr_end, start);
        },
        '{' => {
            self.index += 1;
            self.pushMode(.expression) catch {
                return self.makeToken(.invalid, start);
            };
            return self.makeToken(.expr_start, start);
        },
        else => {
            // Consume text until { or }
            while (self.index < self.buffer.len) {
                const ch = self.buffer[self.index];
                if (ch == '{' or ch == '}' or ch == 0) break;
                self.index += 1;
            }
            return self.makeToken(.text, start);
        },
    }
}

fn nextInlineCode(self: *Tokenizer) Token {
    const start = self.index;

    if (self.index >= self.buffer.len) {
        return .{ .tag = .eof, .loc = .{ .start = @intCast(self.index), .end = @intCast(self.index) } };
    }

    const c = self.buffer[self.index];

    switch (c) {
        0 => return self.makeToken(.eof, start),
        '`' => {
            self.index += 1;
            self.popMode();
            return self.makeToken(.code_inline_end, start);
        },
        else => {
            // Consume text until backtick
            while (self.index < self.buffer.len) {
                const ch = self.buffer[self.index];
                if (ch == '`' or ch == 0) break;
                self.index += 1;
            }
            return self.makeToken(.text, start);
        },
    }
}

fn nextCodeBlock(self: *Tokenizer) Token {
    const start = self.index;

    if (self.index >= self.buffer.len) {
        return .{ .tag = .eof, .loc = .{ .start = @intCast(self.index), .end = @intCast(self.index) } };
    }

    const c = self.buffer[self.index];

    // Check for closing fence at start of line
    if (self.index == self.line_start and c == '`' and self.peekAhead("```")) {
        self.index += 3;
        self.popMode();
        return self.makeToken(.code_fence_end, start);
    }

    switch (c) {
        0 => return self.makeToken(.eof, start),
        '\n' => {
            self.index += 1;
            self.line_start = self.index;
            return self.makeToken(.newline, start);
        },
        else => {
            // Consume text until newline or closing fence
            while (self.index < self.buffer.len) {
                const ch = self.buffer[self.index];
                if (ch == '\n' or ch == 0) break;
                // Check if we're at closing fence
                if (self.index == self.line_start and ch == '`' and self.peekAhead("```")) break;
                self.index += 1;
            }
            return self.makeToken(.text, start);
        },
    }
}

fn isJsxStart(self: *Tokenizer) bool {
    if (self.index + 1 >= self.buffer.len) return false;

    const next_char = self.buffer[self.index + 1];

    // Check for closing tag </
    if (next_char == '/') return true;

    // Check for fragment <>
    if (next_char == '>') return true;

    // Check for component name (uppercase or lowercase identifier)
    return switch (next_char) {
        'a'...'z', 'A'...'Z', '_' => true,
        else => false,
    };
}

fn peekAhead(self: *Tokenizer, needle: []const u8) bool {
    if (self.index + needle.len > self.buffer.len) return false;
    return std.mem.eql(u8, self.buffer[self.index .. self.index + needle.len], needle);
}

fn makeToken(self: *Tokenizer, tag: Token.Tag, start: u32) Token {
    return .{
        .tag = tag,
        .loc = .{
            .start = start,
            .end = @intCast(self.index),
        },
    };
}

fn pushMode(self: *Tokenizer, mode: Mode) !void {
    try self.mode_stack.append(self.allocator, self.mode);
    self.mode = mode;
}

fn popMode(self: *Tokenizer) void {
    if (self.mode_stack.items.len > 0) {
        self.mode = self.mode_stack.pop() orelse .markdown;
    } else {
        self.mode = .markdown;
    }
}

// Tests
test "tokenize heading" {
    const source = "# Hello World\n";
    var tokenizer = init(source, std.testing.allocator);
    defer tokenizer.deinit();

    const tok1 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.heading_start, tok1.tag);

    const tok2 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.text, tok2.tag);
    try std.testing.expectEqualStrings("Hello World", source[tok2.loc.start..tok2.loc.end]);

    const tok3 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.newline, tok3.tag);
}

test "tokenize JSX element" {
    const source = "<Component />";
    var tokenizer = init(source, std.testing.allocator);
    defer tokenizer.deinit();

    const tok1 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.jsx_tag_start, tok1.tag);

    const tok2 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.jsx_identifier, tok2.tag);
    try std.testing.expectEqualStrings("Component", source[tok2.loc.start..tok2.loc.end]);

    const tok3 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.jsx_self_close, tok3.tag);
}

test "tokenize expression" {
    const source = "{state.count}";
    var tokenizer = init(source, std.testing.allocator);
    defer tokenizer.deinit();

    const tok1 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.expr_start, tok1.tag);

    const tok2 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.text, tok2.tag);
    try std.testing.expectEqualStrings("state.count", source[tok2.loc.start..tok2.loc.end]);

    const tok3 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.expr_end, tok3.tag);
}

test "tokenize frontmatter" {
    const source = "---\ntitle: Hello\n---\n";
    var tokenizer = init(source, std.testing.allocator);
    defer tokenizer.deinit();

    const tok1 = tokenizer.next();
    try std.testing.expectEqual(Token.Tag.frontmatter_start, tok1.tag);
}
