import { defineConfig } from 'tsup'

export default defineConfig({
    clean: true,
    dts: true,
    entry: ['src/index.ts'],
    external: [],
    format: ['esm'],
    sourcemap: true,
    target: 'es2024',
    treeshake: true,
})
