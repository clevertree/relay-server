// Main entry point for hook-transpiler
export * from './transpiler';

// Platform-specific exports
export { initWasm, transpileJsx as transpileJsxWasm } from './wasm';
