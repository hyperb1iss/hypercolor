'use client'

import { useEffect, useRef } from 'react'

interface ShaderCanvasProps {
  fragmentShader: string
  uniforms?: Record<string, number | [number, number] | [number, number, number]>
  className?: string
  speed?: number
}

const VERTEX_SHADER = `#version 300 es
in vec4 aPosition;
void main() {
  gl_Position = aPosition;
}
`

function createGL(canvas: HTMLCanvasElement, fragmentShader: string) {
  const gl = canvas.getContext('webgl2', { antialias: false, alpha: false })
  if (!gl) return null

  const vs = gl.createShader(gl.VERTEX_SHADER)
  if (!vs) return null
  gl.shaderSource(vs, VERTEX_SHADER)
  gl.compileShader(vs)

  const fs = gl.createShader(gl.FRAGMENT_SHADER)
  if (!fs) return null
  gl.shaderSource(fs, fragmentShader)
  gl.compileShader(fs)

  if (!gl.getShaderParameter(fs, gl.COMPILE_STATUS)) {
    console.error('Fragment shader error:', gl.getShaderInfoLog(fs))
    return null
  }

  const program = gl.createProgram()
  if (!program) return null
  gl.attachShader(program, vs)
  gl.attachShader(program, fs)
  gl.linkProgram(program)

  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.error('Program link error:', gl.getProgramInfoLog(program))
    return null
  }

  const quad = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1])
  const buf = gl.createBuffer()
  gl.bindBuffer(gl.ARRAY_BUFFER, buf)
  gl.bufferData(gl.ARRAY_BUFFER, quad, gl.STATIC_DRAW)

  const aPos = gl.getAttribLocation(program, 'aPosition')
  gl.enableVertexAttribArray(aPos)
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0)

  // biome-ignore lint/correctness/useHookAtTopLevel: WebGL API, not a React hook
  gl.useProgram(program)
  return { gl, program }
}

export function ShaderCanvas({ fragmentShader, uniforms = {}, className = '', speed = 1 }: ShaderCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const rafRef = useRef<number>(0)
  const startTimeRef = useRef(0)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio, 2)
      const rect = canvas.getBoundingClientRect()
      canvas.width = rect.width * dpr
      canvas.height = rect.height * dpr
    }

    resize()
    const ctx = createGL(canvas, fragmentShader)
    if (!ctx) return

    const { gl, program } = ctx
    startTimeRef.current = performance.now()

    const resizeObserver = new ResizeObserver(resize)
    resizeObserver.observe(canvas)

    const render = () => {
      const elapsed = (performance.now() - startTimeRef.current) / 1000
      gl.viewport(0, 0, canvas.width, canvas.height)

      const timeLoc = gl.getUniformLocation(program, 'iTime')
      const resLoc = gl.getUniformLocation(program, 'iResolution')
      gl.uniform1f(timeLoc, elapsed * speed)
      gl.uniform2f(resLoc, canvas.width, canvas.height)

      for (const [name, value] of Object.entries(uniforms)) {
        const loc = gl.getUniformLocation(program, name)
        if (!loc) continue
        if (typeof value === 'number') {
          if (Number.isInteger(value) && name.startsWith('i') && name[1] === name[1]?.toUpperCase()) {
            gl.uniform1i(loc, value)
          } else {
            gl.uniform1f(loc, value)
          }
        } else if (value.length === 2) {
          gl.uniform2f(loc, value[0], value[1])
        } else if (value.length === 3) {
          gl.uniform3f(loc, value[0], value[1], value[2])
        }
      }

      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4)
      rafRef.current = requestAnimationFrame(render)
    }

    rafRef.current = requestAnimationFrame(render)

    return () => {
      cancelAnimationFrame(rafRef.current)
      resizeObserver.disconnect()
    }
  }, [fragmentShader, uniforms, speed])

  return <canvas className={`block h-full w-full ${className}`} ref={canvasRef} />
}
