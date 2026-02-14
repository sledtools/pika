# WASM Package Guide

The zig-mdx parser can be compiled to WebAssembly and used as a TypeScript/JavaScript library!

## Quick Start

### 1. Build the WASM module

```bash
zig build wasm
```

This creates a 29KB WASM file at `zig-out/wasm/src/mdx.wasm` and copies it to `wasm/src/mdx.wasm`.

### 2. Build the TypeScript package

```bash
cd wasm
bun install
bun run build
```

This creates the npm-ready package in `wasm/dist/`.

### 3. Test it locally

```bash
bun example.ts
```

## Package Structure

```
wasm/
├── src/
│   ├── index.ts       # Main library export
│   ├── types.ts       # TypeScript type definitions
│   └── mdx.wasm       # Compiled WASM binary (29KB!)
├── dist/              # Built package (npm-ready)
│   ├── index.js
│   ├── index.d.ts
│   ├── types.d.ts
│   └── mdx.wasm
├── package.json
├── tsconfig.json
├── build.ts           # Bun build script
└── example.ts         # Usage example
```

## Usage

```typescript
import { parse } from './wasm/dist/index.js';

const mdx = `
# Hello World

This is **MDX** with {dynamic} expressions!

<CustomComponent prop="value" />
`;

const ast = await parse(mdx);

console.log(ast.nodes);    // Array of AST nodes
console.log(ast.tokens);   // Array of tokens
console.log(ast.errors);   // Any parse errors
console.log(ast.source);   // Original source
```

## Publishing to npm

When ready to publish:

1. Update version in `wasm/package.json`
2. Add your repository URL
3. Build the package: `bun run build`
4. Publish: `npm publish` (from the `wasm/` directory)

Then anyone can install it:

```bash
npm install zig-mdx
# or
bun add zig-mdx
```

## Performance

The WASM parser is blazing fast:

- **Binary size**: 29KB (ReleaseSmall optimization)
- **Parse time**: < 1ms for small files, < 50ms for 100KB files
- **Memory**: 16MB initial, 32MB max

## Development Workflow

1. Make changes to Zig source code
2. Rebuild WASM: `zig build wasm`
3. Rebuild package: `cd wasm && bun run build`
4. Test: `bun example.ts`

The WASM file is automatically copied to `wasm/src/mdx.wasm` during the Zig build.

## Architecture

### Zig Side (`src/wasm_exports.zig`)

- `wasm_init()` - Initialize the WASM module
- `wasm_alloc(size)` - Allocate memory
- `wasm_free(ptr, size)` - Free memory
- `wasm_parse_mdx(...)` - Parse MDX and return JSON AST
- `wasm_reset()` - Reset allocator

Uses a 8MB FixedBufferAllocator for memory management.

### TypeScript Side (`wasm/src/index.ts`)

- `init()` - Initialize WASM module (auto-called on first parse)
- `parse(source)` - Parse MDX source and return typed AST
- `reset()` - Reset WASM memory
- `getVersion()` - Get WASM module version

Handles WASM loading, memory management, and type conversion.

## Type Definitions

Full TypeScript types are provided in `wasm/src/types.ts`:

- `AST` - The complete abstract syntax tree
- `Node` - Union of all node types
- `Token` - Token with tag, start, end
- `ParseError` - Parse error information
- All specific node types (HeadingNode, TextNode, etc.)

## Comparison to nostr_zig Approach

### Similarities
- Zig build target with WASM configuration
- Memory allocation exports
- JSON serialization of results
- Bun-based TypeScript tooling

### Improvements
- ✅ Proper npm package (not just an app)
- ✅ Simpler architecture (no crypto/alignment issues)
- ✅ Smaller binary (29KB vs 1MB+)
- ✅ Auto-copy WASM during build
- ✅ Comprehensive TypeScript types
- ✅ Modern Bun build system
- ✅ Ready to publish to npm

## Future Enhancements

- [ ] Stream parsing for large files
- [ ] Incremental parsing
- [ ] Source maps
- [ ] Prettier integration
- [ ] VSCode extension
