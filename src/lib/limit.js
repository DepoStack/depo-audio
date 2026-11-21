// Run async tasks with at most `limit` in flight at once. Tasks start in the
// order they were scheduled. A task that throws does not stop the others; its
// rejection goes to the caller of `run`.
export function createLimiter(limit) {
  if (!Number.isInteger(limit) || limit < 1) throw new Error('limit must be a positive integer')
  let active = 0
  const waiting = []

  const next = () => {
    if (active >= limit || waiting.length === 0) return
    const { task, resolve, reject } = waiting.shift()
    active += 1
    Promise.resolve()
      .then(task)
      .then(resolve, reject)
      .finally(() => {
        active -= 1
        next()
      })
  }

  const run = task =>
    new Promise((resolve, reject) => {
      waiting.push({ task, resolve, reject })
      next()
    })

  // Drop tasks that have not started yet (their `run` resolves to null);
  // running ones finish.
  const clearPending = () => {
    for (const { resolve } of waiting.splice(0)) resolve(null)
  }

  return {
    run,
    clearPending,
    get active() {
      return active
    },
    get pending() {
      return waiting.length
    },
  }
}
