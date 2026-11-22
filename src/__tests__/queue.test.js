import { describe, expect, it } from 'vitest'
import {
  TRM_OPEN_ENDED_NOTICE,
  TRM_SESSION_FIRST_MISSING,
  TRM_SESSION_LAST_MISSING,
  audioTrackNotice,
  catJobPaths,
  mergeQueuedFiles,
  partitionQueuePaths,
  reexportConfirmCopy,
  trmRecordingNotice,
  trmSessionNotices,
} from '../lib/queue'
import { ftrSessionKey } from '../utils'

describe('partitionQueuePaths', () => {
  it('separates TRS companions without discarding any dropped TRM chunks', () => {
    expect(
      partitionQueuePaths([
        '/case/session.TRS',
        '/case/chunk-2.trm',
        { path: '/case/chunk-1.trm' },
        '/case/chunk-2.trm',
      ]),
    ).toEqual({
      queuePaths: ['/case/chunk-2.trm', '/case/chunk-1.trm'],
      trsCompanions: ['/case/session.TRS'],
    })
  })
})

describe('mergeQueuedFiles', () => {
  const early = { path: '/case/CR24_20180621-1449_01d4096f0757ee50.trm', name: 'early.trm' }
  const middle = { path: '/case/CR24_20180621-1454_01d4096fbaa19b00.trm', name: 'middle.trm' }
  const late = { path: '/case/CR24_20180621-1459_01d409706d824cb0.trm', name: 'late.trm' }

  it('dedupes and chronologically sorts the combined FTR queue across additions', () => {
    const merged = mergeQueuedFiles([late], [middle, early, late])
    expect(merged.map(file => file.path)).toEqual([early.path, middle.path, late.path])
  })

  it('can atomically replace an existing queue', () => {
    const merged = mergeQueuedFiles([{ path: '/old.wav' }], [late, early], { replace: true })
    expect(merged.map(file => file.path)).toEqual([early.path, late.path])
  })
})

describe('catJobPaths', () => {
  it('returns every valid file path in a detected court-software job', () => {
    expect(catJobPaths({ files: [{ path: '/one.trm' }, { path: '/two.trm' }, {}] })).toEqual(['/one.trm', '/two.trm'])
  })
})

describe('reexportConfirmCopy', () => {
  it('counts the queue and the Library files with the right number', () => {
    const many = reexportConfirmCopy({ queued: 3, queuedAtOpen: 3, incoming: 2 })
    expect(many.title).toBe('Replace the 3 files in the queue?')
    expect(many.confirmLabel).toBe('Replace 3 files')
    expect(many.description).toBe(
      'The 3 files waiting in Convert are taken out of the queue. Nothing is deleted from disk. Add to queue keeps them and adds the 2 Library files after them.',
    )
    expect(many.destructive).toBe(true)

    const one = reexportConfirmCopy({ queued: 1, queuedAtOpen: 1, incoming: 1 })
    expect(one.title).toBe('Replace the 1 file in the queue?')
    expect(one.description).not.toMatch(/\bfiles\b/)
  })

  it('names files added while the dialog was open', () => {
    expect(reexportConfirmCopy({ queued: 4, queuedAtOpen: 2, incoming: 1 }).description).toContain(
      '2 files were added to the queue while this was open and are counted here.',
    )
    expect(reexportConfirmCopy({ queued: 1, queuedAtOpen: 2, incoming: 1 }).description).not.toContain('added to the')
  })
})

describe('audioTrackNotice', () => {
  const stream = (language, title, channels = 2) => ({ channels, sampleRate: 48000, language, title })

  it('names the converted track with its language and title', () => {
    expect(
      audioTrackNotice({ convertedIndex: 0, streams: [stream('eng', 'Floor', 1), stream('spa', 'Interpreter', 4)] }),
    ).toBe('This file has 2 audio tracks; converting track 1 (eng, "Floor"). The other track is not converted.')
  })

  it('leaves out tags the track does not have', () => {
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream(null, null), stream(null, null), stream()] })).toBe(
      'This file has 3 audio tracks; converting track 1. The other 2 tracks are not converted.',
    )
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream(null, 'Room 4'), stream('eng', null)] })).toContain(
      'converting track 1 ("Room 4")',
    )
  })

  it('says nothing for one track or no probe answer', () => {
    expect(audioTrackNotice({ convertedIndex: 0, streams: [stream('eng', 'Floor')] })).toBe('')
    expect(audioTrackNotice(null)).toBe('')
    expect(audioTrackNotice(undefined)).toBe('')
    expect(audioTrackNotice([])).toBe('')
  })
})

describe('FTR recording notices', () => {
  const route = (sessionFirst, sessionLast, openEnded = false) => ({
    convertedIndex: 0,
    streams: [{ channels: 4 }],
    trm: { sessionFirst, sessionLast, openEnded },
  })
  const chunk = (location, time, filetime, ext = 'trm') => `/court/${location}_20260105-${time}_${filetime}.${ext}`
  const a = chunk('CR4', '0900', '01dc5f0000000001')
  const b = chunk('CR4', '0905', '01dc5f0000000002', 'tll')
  const c = chunk('CR4', '0910', '01dc5f0000000003')

  it('names an unclosed recording and nothing else', () => {
    expect(trmRecordingNotice(route(true, true, true))).toBe(TRM_OPEN_ENDED_NOTICE)
    expect(TRM_OPEN_ENDED_NOTICE).toBe(
      'This recording was not closed properly; DepoAudio read the audio that was written.',
    )
    expect(trmRecordingNotice(route(true, true))).toBe('')
    expect(trmRecordingNotice({ streams: [] })).toBe('')
    expect(trmRecordingNotice(null)).toBe('')
  })

  it('groups chunks by folder, location and date', () => {
    expect(ftrSessionKey(a)).toBe(ftrSessionKey(b))
    expect(ftrSessionKey(a)).not.toBe(ftrSessionKey(chunk('CR5', '0900', '01dc5f0000000001')))
    expect(ftrSessionKey(a)).not.toBe(ftrSessionKey(a.replace('/court/', '/other/')))
    expect(ftrSessionKey('C:\\court\\CR4_20260105-0900_01dc5f0000000001.TRM')).toBe(
      ftrSessionKey('C:/court/CR4_20260105-0900_01dc5f0000000009.trm'),
    )
    expect(ftrSessionKey('/court/session part 1.trm')).toBeNull()
  })

  it('says which end of a session is missing, on the first queued chunk', () => {
    const files = [a, b, c].map(path => ({ path }))
    expect(
      trmSessionNotices(files, { [a]: route(true, false), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({})
    expect(
      trmSessionNotices(files, { [a]: route(false, false), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({ [a]: TRM_SESSION_FIRST_MISSING })
    expect(
      trmSessionNotices(files, { [a]: route(true, false), [b]: route(false, false), [c]: route(false, false) }),
    ).toEqual({ [a]: TRM_SESSION_LAST_MISSING })
    expect(trmSessionNotices([{ path: b }], { [b]: route(false, false) })).toEqual({
      [b]: `${TRM_SESSION_FIRST_MISSING} ${TRM_SESSION_LAST_MISSING}`,
    })
    expect(TRM_SESSION_FIRST_MISSING).toBe("This session's first file isn't in the queue.")
  })

  it('claims nothing until every chunk of the session has been read', () => {
    const files = [a, b].map(path => ({ path }))
    expect(trmSessionNotices(files, { [a]: route(false, false) })).toEqual({})
    expect(trmSessionNotices([{ path: '/court/renamed.trm' }], { '/court/renamed.trm': route(false, false) })).toEqual(
      {},
    )
    expect(trmSessionNotices([{ path: '/case/hearing.wav' }], {})).toEqual({})
  })
})
