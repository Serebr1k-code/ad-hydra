/** Incremental SSE framing. Never retain an unbounded partial event. */
export class EventFrames {
  private pending = ''
  push(chunk: string): string[] {
    this.pending += chunk
    const frames: string[] = []
    let boundary: number
    while ((boundary = this.pending.indexOf('\n\n')) >= 0) {
      if (boundary > 131072) throw new Error('Live event limit exceeded')
      frames.push(this.pending.slice(0, boundary))
      this.pending = this.pending.slice(boundary + 2)
    }
    if (this.pending.length > 131072) throw new Error('Live event limit exceeded')
    return frames
  }
}

/** Core sends explicit named traffic/loss events; comments are keepalives only. */
export function isEventFrame(frame: string): boolean {
 return frame.split('\n').some(line => line.startsWith('event:'))
}
