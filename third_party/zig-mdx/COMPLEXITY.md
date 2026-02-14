# Complexity Notes: Markdown Inside JSX

This document captures challenges we encountered with parsing and rendering markdown content inside JSX blocks. These are areas where the current implementation required tricky workarounds and may benefit from future simplification.

## The Core Problem

MDX mixes two syntaxes with different whitespace semantics:

1. **Markdown**: Whitespace is significant (blank lines = paragraph breaks, trailing spaces = hard breaks)
2. **JSX**: Whitespace is typically insignificant (formatting only)

When markdown content appears inside JSX elements, we need to decide which rules apply.

## Current Approach

We treat JSX as "layout containers" that can hold markdown-like content:

```mdx
<VStack>
{msg.content}

{msg.created_at}
</VStack>
```

The blank line between expressions is preserved in output. But this required several workarounds.

## Challenge 1: Block Separation

**Problem**: At the top level, blank lines separate blocks. Inside JSX, should they?

**Original issue**: The parser was creating empty paragraph nodes for blank lines, which caused:
- Extra blank lines in output
- Inconsistent roundtripping

**Current solution** (Parser.zig `parseDocument`):
- Skip both `.newline` AND `.blank_line` tokens between top-level blocks
- Inside JSX children, skip them too but use heuristics in renderer

**Renderer heuristic** (Render.zig):
- `isContentBlock()` identifies "content" nodes (expressions, paragraphs, etc.)
- Blank lines added between consecutive content blocks in JSX
- But NOT between JSX structural elements like `<Profile />` and `<Text>`

**Limitation**: We can't distinguish "one blank line" from "three blank lines" - all get normalized to one.

## Challenge 2: Hard Breaks

**Problem**: Hard breaks (`two spaces + newline`) should connect content, not separate it.

**Original issue**: For `{a}  \n{b}`:
- Parser created: `[flow_expression, paragraph: [hard_break, text_expression]]`
- The hard_break at the START of a paragraph is semantically meaningless

**Current solution** (Parser.zig `parseBlock`):
- Removed special case for `expr_start => parseFlowExpression()`
- All expressions fall through to `parseParagraph()`
- This allows `{a}  \n{b}` to become one paragraph: `[text_expression, hard_break, text_expression]`

**Trade-off**: We no longer create `mdx_flow_expression` at the top level. All top-level expressions are `mdx_text_expression` inside paragraphs.

## Challenge 3: Hard Breaks in JSX Children

**Problem**: When rendering JSX children, each child normally gets its own line. But hard_break should attach to adjacent content.

**Original issue**: For `<V>{a}  \n{b}</V>`:
- Renderer output: `{a}\n  \n\n{b}` (wrong - extra newlines)
- Expected: `{a}  \n{b}` (hard break connects them)

**Current solution** (Render.zig JSX element rendering):
- Look ahead: if next child is `hard_break`, don't add newline after current child
- Don't add newline after `hard_break` itself (it includes one)

```zig
const next_is_hard_break = if (i + 1 < children.len)
    ast.nodes.get(children[i + 1]).tag == .hard_break
else
    false;

if (child.tag != .hard_break and !next_is_hard_break) {
    try writer.writeByte('\n');
}
```

**This is ugly** - we're doing lookahead in the renderer to work around AST structure.

## Potential Future Improvements

### Option A: Richer AST

Store whitespace/break information directly in the AST:
- Add `preceding_blank_lines: u8` field to nodes
- Or create explicit "break" nodes that carry their context

**Pros**: Exact roundtripping, cleaner renderer
**Cons**: Larger AST, more complex parser

### Option B: Paragraph Wrapping in JSX

Wrap JSX children in implicit paragraphs when they contain inline content:

```
<VStack>
  {a}  \n{b}     ->  paragraph: [expr, hard_break, expr]

  {c}            ->  paragraph: [expr]
</VStack>
```

**Pros**: Consistent with top-level parsing
**Cons**: Changes AST structure, may break consumer expectations

### Option C: "MDX Mode" for JSX

Add an attribute or convention for JSX elements that contain markdown:

```mdx
<VStack mdx>
  {a}
  {b}
</VStack>
```

Content inside `mdx` containers gets full markdown parsing; others get JSX semantics.

**Pros**: Explicit, user-controlled
**Cons**: Non-standard, adds syntax

## Summary of Workarounds

| Location | Workaround | Why Needed |
|----------|------------|------------|
| Parser.zig `parseDocument` | Skip `.newline` AND `.blank_line` | Prevent empty paragraphs |
| Parser.zig `parseBlock` | No special case for `expr_start` | Hard breaks in expressions |
| Render.zig `isContentBlock` | Heuristic for blank lines | Content vs structure distinction |
| Render.zig JSX children | Lookahead for `hard_break` | Attach hard breaks to content |

## Files Involved

- `src/Parser.zig`: Block parsing, expression handling
- `src/Render.zig`: JSX children rendering, hard break handling
- `src/Tokenizer.zig`: Token generation (newline, blank_line, hard_break)

## Test Cases to Maintain

```mdx
# Hard break between expressions
{a}
{b}
# Should roundtrip as one paragraph with hard_break

# Blank line between expressions
{a}

{b}
# Should roundtrip as two separate paragraphs

# Hard break inside JSX
<V>
{a}
{b}
</V>
# Should preserve hard break, not add extra newlines

# Mixed JSX children
<V>
  <Profile />
  <Text>{content}</Text>
</V>
# No blank lines between JSX elements
```
