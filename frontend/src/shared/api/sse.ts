/** Incremental SSE framing. Never retain an unbounded partial event. */
export class EventFrames {
  private pending = ''
  push(chunk: string): string[] {
    this.pending += chunk
    const frames: string[] = []
    let boundary: number
    while ((boundary = this.pending.indexOf('\n\n')) >= 0) {
      if (boundary > 262144) throw new Error('Live event limit exceeded')
      frames.push(this.pending.slice(0, boundary))
      this.pending = this.pending.slice(boundary + 2)
    }
    if (this.pending.length > 262144) throw new Error('Live event limit exceeded')
    return frames
  }
}

/** Core sends explicit named traffic/loss events; comments are keepalives only. */
export function isEventFrame(frame: string): boolean {
 return frame.split('\n').some(line => line.startsWith('event:'))
}

export function parseEventFrame(frame:string):{event:string;data:string}|undefined {
 let event='message';const data:string[]=[]
 for(const line of frame.split(/\r?\n/)) {
  if(line.startsWith(':'))continue
  const colon=line.indexOf(':')
  const key=colon<0?line:line.slice(0,colon)
  let value=colon<0?'':line.slice(colon+1)
  if(value.startsWith(' '))value=value.slice(1)
  if(key==='event')event=value
  if(key==='data')data.push(value)
 }
 return data.length?{event,data:data.join('\n')}:undefined
}
