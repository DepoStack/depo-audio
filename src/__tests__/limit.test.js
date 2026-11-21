import { describe, expect, it } from 'vitest'
import { createLimiter } from '../lib/limit'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((ok, fail) => {
    resolve = ok
    reject = fail
  })
  return { promise, resolve, reject }
}

const flush = () => new Promise(resolve => setTimeout(resolve, 0))

describe('createLimiter', () => {
  it('runs at most `limit` tasks at once, in order', async () => {
    const limiter = createLimiter(2)
    const gates = [deferred(), deferred(), deferred(), deferred()]
    const started = []
    const results = gates.map((gate, index) =>
      limiter.run(() => {
        started.push(index)
        return gate.promise
      }),
    )
    const firstOutcome = results[0].catch(error => error)
    await flush()
    expect(started).toEqual([0, 1])
    expect(limiter.active).toBe(2)
    expect(limiter.pending).toBe(2)

    gates[1].resolve('b')
    await flush()
    expect(started).toEqual([0, 1, 2])
    gates[0].reject(new Error('probe failed'))
    await flush()
    expect(started).toEqual([0, 1, 2, 3])
    gates[2].resolve('c')
    gates[3].resolve('d')
    expect((await firstOutcome).message).toBe('probe failed')
    await expect(Promise.all(results.slice(1))).resolves.toEqual(['b', 'c', 'd'])
    expect(limiter.active).toBe(0)
  })

  it('drops tasks that have not started when cleared', async () => {
    const limiter = createLimiter(1)
    const gate = deferred()
    const started = []
    const first = limiter.run(() => {
      started.push('first')
      return gate.promise
    })
    const second = limiter.run(() => started.push('second'))
    await flush()
    limiter.clearPending()
    await expect(second).resolves.toBeNull()
    gate.resolve('done')
    await expect(first).resolves.toBe('done')
    await flush()
    expect(started).toEqual(['first'])
  })

  it('rejects a limit that is not a positive integer', () => {
    expect(() => createLimiter(0)).toThrow()
    expect(() => createLimiter(1.5)).toThrow()
  })
})
