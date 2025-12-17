export { transpileJsx } from './wasm';

export interface TranspileOptions {
    filename?: string;
    sourceMap?: boolean;
}

export interface TranspileResult {
    code: string;
    map?: string;
}
