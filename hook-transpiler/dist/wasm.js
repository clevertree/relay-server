import init, { transpile_jsx } from '../wasm/hook_transpiler.js';

let wasmInitialized = false;

export async function initWasm(wasmUrl) {
    if (wasmInitialized) return;

    if (wasmUrl) {
        await init(wasmUrl);
    } else {
        await init();
    }

    wasmInitialized = true;
}

export async function transpileJsx(source, options = {}) {
    if (!wasmInitialized) {
        await initWasm();
    }

    const result = transpile_jsx(source, options.filename || 'module.jsx');
    return JSON.parse(result);
}
