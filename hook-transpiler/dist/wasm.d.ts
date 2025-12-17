export interface TranspileOptions {
    filename?: string;
    sourceMap?: boolean;
}

export interface TranspileResult {
    code: string;
    map?: string;
}

export function initWasm(wasmUrl?: string): Promise<void>;
export function transpileJsx(source: string, options?: TranspileOptions): Promise<TranspileResult>;
