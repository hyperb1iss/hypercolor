import { basename, dirname, resolve } from 'node:path'

import type { ExtractedArtifactMetadata } from './types'

export async function extractArtifactMetadata(entryPath: string): Promise<ExtractedArtifactMetadata> {
    const workerExtension = import.meta.url.endsWith('.ts') ? 'ts' : 'js'
    const workerUrl = new URL(`./metadata-worker.${workerExtension}`, import.meta.url)

    return await new Promise<ExtractedArtifactMetadata>((resolveMetadata, rejectMetadata) => {
        const worker = new Worker(workerUrl.href, { type: 'module' })

        const finish = async (callback: () => void) => {
            callback()
            await worker.terminate()
        }

        worker.onerror = (event) => {
            void finish(() => {
                rejectMetadata(event.error ?? new Error(event.message))
            })
        }

        worker.onmessage = (event: MessageEvent<{ error?: string; metadata?: ExtractedArtifactMetadata }>) => {
            void finish(() => {
                if (event.data.metadata) {
                    resolveMetadata(event.data.metadata)
                    return
                }

                rejectMetadata(new Error(event.data.error ?? `Metadata extraction failed for ${entryPath}`))
            })
        }

        worker.postMessage({ entryPath: resolve(entryPath) })
    })
}

export function artifactIdFromEntry(entryPath: string): string {
    return basename(dirname(entryPath))
}
