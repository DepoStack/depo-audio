import { beforeEach, describe, expect, it } from 'vitest'
import {
  WEB_STORAGE_KEYS,
  clearWebData,
  collectWebData,
  defaultBackupName,
  restoreWebData,
  summarizeWebData,
} from '../lib/localData'
import { removeStoredSegments, saveStoredSegments, storageKey, syncKey } from '../lib/transcript'

// Synthetic data only: no real case content.
const lineA = { id: 'a', start: 1, speaker: 'Q', text: 'Synthetic question' }
const lineB = { id: 'b', start: 2, speaker: 'A', text: 'Synthetic answer' }

function memoryStorage(initial = {}) {
  const data = new Map(Object.entries(initial))
  return {
    data,
    get length() {
      return data.size
    },
    key: index => [...data.keys()][index] ?? null,
    getItem: key => (data.has(key) ? data.get(key) : null),
    setItem: (key, value) => data.set(key, String(value)),
    removeItem: key => data.delete(key),
  }
}

describe('transcript storage removal', () => {
  it('deletes the stored transcript for one track only', () => {
    const storage = memoryStorage({
      [storageKey('/a.wav')]: JSON.stringify([lineA]),
      [storageKey('/b.wav')]: JSON.stringify([lineB]),
    })
    expect(removeStoredSegments(storage, '/a.wav')).toBe(true)
    expect(storage.getItem(storageKey('/a.wav'))).toBeNull()
    expect(storage.getItem(storageKey('/b.wav'))).not.toBeNull()
  })

  it('stores no key for an empty transcript, but still probes that saving works', () => {
    const storage = memoryStorage({ [storageKey('/a.wav')]: JSON.stringify([lineA]) })
    expect(saveStoredSegments(storage, '/a.wav', [])).toBe(true)
    expect(storage.data.size).toBe(0)

    const blocked = {
      ...memoryStorage(),
      setItem: () => {
        throw new Error('quota')
      },
    }
    expect(saveStoredSegments(blocked, '/a.wav', [])).toBe(false)
  })

  it('reports a failed removal', () => {
    const blocked = {
      removeItem: () => {
        throw new Error('blocked')
      },
    }
    expect(removeStoredSegments(blocked, '/a.wav')).toBe(false)
    expect(removeStoredSegments(null, '/a.wav')).toBe(false)
  })
})

describe('collecting web data for a backup', () => {
  it('carries proofing flags in the transcripts a backup collects', () => {
    const flagged = { ...lineA, flag: { note: 'check' } }
    const storage = memoryStorage({ [storageKey('/a.wav')]: JSON.stringify([flagged]) })
    expect(collectWebData(storage).transcripts['/a.wav']).toEqual([flagged])
  })

  it('collects non-empty transcripts and valid bookmarks only', () => {
    const storage = memoryStorage({
      [storageKey('/a.wav')]: JSON.stringify([lineA]),
      [storageKey('/empty.wav')]: '[]',
      [storageKey('/corrupt.wav')]: '{not json',
      'player-bookmarks': JSON.stringify([{ time: 3, trackPath: '/a.wav', label: '0:03' }, { bad: true }]),
      'player-speed': '1.5',
      unrelated: 'x',
    })

    const collected = collectWebData(storage)
    expect(Object.keys(collected.transcripts)).toEqual(['/a.wav'])
    expect(collected.transcripts['/a.wav']).toEqual([lineA])
    expect(collected.bookmarks).toEqual([{ time: 3, trackPath: '/a.wav', label: '0:03' }])
    expect(summarizeWebData(storage)).toEqual({ transcripts: 1, bookmarks: 1 })
  })

  it('treats unavailable storage as empty', () => {
    const blocked = {
      get length() {
        throw new Error('blocked')
      },
    }
    expect(collectWebData(blocked)).toEqual({ transcripts: {}, bookmarks: [] })
    expect(collectWebData(null)).toEqual({ transcripts: {}, bookmarks: [] })
  })

  it('names the backup by date', () => {
    expect(defaultBackupName(new Date('2026-11-13T23:00:00Z'))).toMatch(/^DepoAudio backup 2026-11-1[34]\.zip$/)
  })
})

describe('clearing web data', () => {
  let storage
  beforeEach(() => {
    storage = memoryStorage({
      [storageKey('/a.wav')]: JSON.stringify([lineA]),
      [storageKey('C:\\cases\\b.wav')]: JSON.stringify([lineB]),
      [syncKey('/a.wav')]: JSON.stringify({ shift: 1.5 }),
      'player-bookmarks': '[]',
      'player-speed': '2',
      'player-backstep': '3',
      'player-volume': '0.8',
      'player-layout': '{"left":260}',
      'depoaudio-theme': 'dark',
      'not-depoaudio': 'kept',
    })
  })

  it('removes every DepoAudio key and nothing else', () => {
    expect(WEB_STORAGE_KEYS).toEqual([
      'player-bookmarks',
      'player-speed',
      'player-backstep',
      'player-volume',
      'player-layout',
      'depoaudio-theme',
    ])
    expect(clearWebData(storage)).toEqual({ removed: 9, failed: 0 })
    expect([...storage.data.keys()]).toEqual(['not-depoaudio'])
  })

  it('counts keys it could not remove', () => {
    storage.removeItem = () => {
      throw new Error('blocked')
    }
    expect(clearWebData(storage)).toEqual({ removed: 0, failed: 9 })
  })
})

describe('restoring web data from a backup', () => {
  it('adds missing transcripts, keeps different local ones, and merges bookmarks', () => {
    const storage = memoryStorage({
      [storageKey('/kept.wav')]: JSON.stringify([lineB]),
      [storageKey('/same.wav')]: JSON.stringify([lineA]),
      'player-bookmarks': JSON.stringify([{ time: 1, trackPath: '/a.wav', label: 'one' }]),
    })

    const result = restoreWebData(storage, {
      transcripts: {
        '/new.wav': [lineA],
        '/kept.wav': [lineA],
        '/same.wav': [lineA],
        '/empty.wav': [],
        '': [lineA],
        '/bad.wav': 'not a list',
      },
      bookmarks: [
        { time: 1, trackPath: '/a.wav', label: 'one' },
        { time: 2, trackPath: '/a.wav', label: 'two' },
        { nonsense: true },
      ],
    })

    expect(result).toEqual({ transcriptsAdded: 1, transcriptsKept: 1, bookmarksAdded: 1, failed: 0 })
    expect(JSON.parse(storage.getItem(storageKey('/new.wav')))).toEqual([lineA])
    expect(JSON.parse(storage.getItem(storageKey('/kept.wav')))).toEqual([lineB])
    expect(storage.getItem(storageKey('/empty.wav'))).toBeNull()
    expect(JSON.parse(storage.getItem('player-bookmarks'))).toEqual([
      { time: 1, trackPath: '/a.wav', label: 'one' },
      { time: 2, trackPath: '/a.wav', label: 'two' },
    ])
  })

  it('counts writes that fail instead of reporting them as restored', () => {
    const storage = {
      ...memoryStorage(),
      setItem: () => {
        throw new Error('quota')
      },
    }
    const result = restoreWebData(storage, {
      transcripts: { '/new.wav': [lineA] },
      bookmarks: [{ time: 2, trackPath: '/a.wav' }],
    })
    expect(result).toEqual({ transcriptsAdded: 0, transcriptsKept: 0, bookmarksAdded: 0, failed: 2 })
  })

  it('ignores a malformed backup payload', () => {
    const storage = memoryStorage()
    expect(restoreWebData(storage, { transcripts: [], bookmarks: {} })).toEqual({
      transcriptsAdded: 0,
      transcriptsKept: 0,
      bookmarksAdded: 0,
      failed: 0,
    })
    expect(storage.data.size).toBe(0)
  })
})
