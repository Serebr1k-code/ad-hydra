interface ReconciliationOptions<T> {
  pending: () => boolean
  paused: () => boolean
  read: () => Promise<T>
  accept: (value: T) => void
  failed: (cause: unknown) => void
  schedule?: (run: () => Promise<void>, delay: number) => () => void
}

/** One bounded request at a time, with invalidation across user mutations and disposal. */
export function createPendingReconciliation<T>(options: ReconciliationOptions<T>) {
  let generation = 0, attempts = 0, inFlight = false, disposed = false
  let cancel: (() => void) | undefined
  const schedule = options.schedule ?? ((run, delay) => {
    const timer = setTimeout(() => { void run() }, delay)
    return () => clearTimeout(timer)
  })
  function enqueue(version: number, delay: number) {
    if (disposed || version !== generation || !options.pending()) return
    cancel?.()
    cancel = schedule(() => poll(version), delay)
  }
  async function poll(version: number) {
    if (disposed || version !== generation) return
    cancel = undefined
    if (!options.pending()) return
    if (inFlight || options.paused()) { enqueue(version, 5000); return }
    inFlight = true
    let failed = false
    try {
      const value = await options.read()
      if (!disposed && version === generation) options.accept(value)
    } catch (cause) {
      failed = true
      if (!disposed && version === generation) options.failed(cause)
    } finally {
      inFlight = false
      attempts++
      enqueue(version, failed || attempts >= 4 ? 5000 : 500)
    }
  }
  function invalidate() { generation++; cancel?.(); cancel = undefined }
  return {
    invalidate,
    resume() { if (disposed) return; attempts = 0; enqueue(generation, 500) },
    stop() { disposed = true; invalidate() },
  }
}
