import { describe, expect, it } from 'vitest'
import {
  TRM_END_BOUNDARY_MISSING,
  TRM_OPEN_ENDED_NOTICE,
  TRM_SIGNED_NOTICE,
  TRM_START_BOUNDARY_MISSING,
  audioTrackNotice,
  catJobPaths,
  mergeQueuedFiles,
  partitionQueuePaths,
  reexportConfirmCopy,
  trmRecordingNotice,
  trmSessionNotices,
} from '../lib/queue'
import { ftrRecordingDate, ftrSessionKey } from '../utils'

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
  const route = (startBoundary, endBoundary, openEnded = false, extra = {}) => ({
    convertedIndex: 0,
    streams: [{ channels: 4 }],
    trm: { startBoundary, endBoundary, openEnded, ...extra },
  })
  const chunk = (location, time, filetime, ext = 'trm') => `/court/${location}_20260105-${time}_${filetime}.${ext}`
  const a = chunk('CR4', '0900', '01dc5f0000000001')
  const b = chunk('CR4', '0905', '01dc5f0000000002', 'tll')
  const c = chunk('CR4', '0910', '01dc5f0000000003')

  it('names an unclosed recording and nothing else', () => {
    expect(trmRecordingNotice(route(true, true, true))).toBe(TRM_OPEN_ENDED_NOTICE)
    expect(TRM_OPEN_ENDED_NOTICE).toBe(
      "This recording's header says it was never closed (or is still recording); DepoAudio read the audio written so far.",
    )
    expect(trmRecordingNotice(route(true, true))).toBe('')
    expect(trmRecordingNotice({ streams: [] })).toBe('')
    expect(trmRecordingNotice(null)).toBe('')
  })

  it('groups chunks by folder and location, with the date read separately', () => {
    expect(ftrSessionKey(a)).toBe(ftrSessionKey(b))
    expect(ftrSessionKey(a)).toBe(ftrSessionKey(a.replace('20260105', '20260106')))
    expect(ftrRecordingDate(a)).toBe('20260105')
    expect(ftrRecordingDate('/court/session part 1.trm')).toBeNull()
    expect(ftrSessionKey(a)).not.toBe(ftrSessionKey(chunk('CR5', '0900', '01dc5f0000000001')))
    expect(ftrSessionKey(a)).not.toBe(ftrSessionKey(a.replace('/court/', '/other/')))
    expect(ftrSessionKey('C:\\court\\CR4_20260105-0900_01dc5f0000000001.TRM')).toBe(
      ftrSessionKey('C:/court/CR4_20260105-0900_01dc5f0000000009.trm'),
    )
    expect(ftrSessionKey('/court/session part 1.trm')).toBeNull()
  })

  it('says which boundary mark is missing, on the earliest queued file', () => {
    const files = [a, b, c].map(path => ({ path }))
    expect(
      trmSessionNotices(files, { [a]: route(true, false), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({})
    expect(
      trmSessionNotices(files, { [a]: route(false, false), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({ [a]: TRM_START_BOUNDARY_MISSING })
    expect(
      trmSessionNotices(files, { [a]: route(true, false), [b]: route(false, false), [c]: route(false, false) }),
    ).toEqual({ [a]: TRM_END_BOUNDARY_MISSING })
    expect(trmSessionNotices([{ path: b }], { [b]: route(false, false) })).toEqual({
      [b]: `${TRM_START_BOUNDARY_MISSING} ${TRM_END_BOUNDARY_MISSING}`,
    })
    expect(TRM_START_BOUNDARY_MISSING).toBe(
      'The earliest queued file of this recording has no start-boundary mark, so FTR would accept earlier files before it; if there are any, add them too.',
    )
    expect(TRM_END_BOUNDARY_MISSING).toBe(
      'The latest queued file of this recording has no end-boundary mark, so FTR would accept later files after it; if there are any, add them too.',
    )
  })

  it('judges the earliest and latest file by recording order, not queue order or any file', () => {
    // Queued out of order: the marks are on the right files once sorted.
    const files = [c, a, b].map(path => ({ path }))
    expect(
      trmSessionNotices(files, { [a]: route(true, false), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({})
    // A start mark on a later file is not a start for the earlier ones.
    expect(
      trmSessionNotices(files, { [a]: route(false, false), [b]: route(true, false), [c]: route(false, true) }),
    ).toEqual({ [a]: `${TRM_START_BOUNDARY_MISSING} ${TRM_END_BOUNDARY_MISSING}` })
  })

  it('cuts a group into recordings at boundary marks', () => {
    const files = [a, b, c].map(path => ({ path }))
    // a is a stand-alone recording; b..c is another with both ends marked.
    expect(
      trmSessionNotices(files, { [a]: route(true, true), [b]: route(true, false), [c]: route(false, true) }),
    ).toEqual({})
    // An end mark on a, then b has no start mark: b's recording may have
    // earlier files that are not queued.
    expect(
      trmSessionNotices(files, { [a]: route(true, true), [b]: route(false, false), [c]: route(false, true) }),
    ).toEqual({ [b]: TRM_START_BOUNDARY_MISSING })
  })

  it('names a signature, a plain codec and a refusal on the file itself', () => {
    expect(trmRecordingNotice(route(true, true, false, { signature: 'signed (not verified)' }))).toBe(TRM_SIGNED_NOTICE)
    expect(TRM_SIGNED_NOTICE).toBe(
      'This recording carries a signature: signed (not verified). DepoAudio does not check signatures.',
    )
    expect(trmRecordingNotice(route(true, true, true, { signature: 'signed (not verified)' }))).toBe(
      `${TRM_OPEN_ENDED_NOTICE} ${TRM_SIGNED_NOTICE}`,
    )
    expect(trmRecordingNotice(route(true, true, false, { audioCodec: 'mp3' }))).toBe(
      'This FTR recording holds MP3 audio, which FFmpeg reads as ordinary audio.',
    )
    expect(trmRecordingNotice(route(true, true, false, { audioCodec: 'ftr-aac' }))).toBe('')
    const refusal =
      "This FTR recording's format isn't one FTR's own player can split, so DepoAudio does not convert it."
    expect(trmRecordingNotice({ convertedIndex: 0, streams: [], trmRefusal: refusal })).toBe(refusal)
  })

  it('keeps a session that runs past midnight together, and splits days apart', () => {
    const night = path => path.replace('20260105', '20260106')
    const before = chunk('CR4', '2350', '01dc5f0000000010')
    const after = night(chunk('CR4', '0005', '01dc5f0000000011'))
    const files = [before, after].map(path => ({ path }))
    expect(trmSessionNotices(files, { [before]: route(true, false), [after]: route(false, true) })).toEqual({})
    // Month and year ends are consecutive days too.
    const dec = '/court/CR4_20261231-2350_01dc5f0000000020.trm'
    const jan = '/court/CR4_20270101-0005_01dc5f0000000021.trm'
    expect(
      trmSessionNotices([{ path: jan }, { path: dec }], { [dec]: route(true, false), [jan]: route(false, true) }),
    ).toEqual({})
    // Two days apart: two groups, each judged on its own.
    const later = before.replace('20260105', '20260107')
    expect(
      trmSessionNotices([{ path: before }, { path: later }], {
        [before]: route(true, true),
        [later]: route(false, true),
      }),
    ).toEqual({ [later]: TRM_START_BOUNDARY_MISSING })
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
