export function fmtSize(b) {
  if (!b || b === 0) return '—'
  if (b < 1048576) return `${(b/1024).toFixed(1)} KB`
  if (b < 1073741824) return `${(b/1048576).toFixed(1)} MB`
  return `${(b/1073741824).toFixed(2)} GB`
}

export function fmtTime(s) {
  if (!s || isNaN(s)) return '0:00'
  const m = Math.floor(s/60), sec = Math.floor(s%60)
  return `${m}:${sec.toString().padStart(2,'0')}`
}

export function basename(p) {
  return (p||'').replace(/\\/g,'/').split('/').pop()
}

// FTR Gold records court sessions in ~5-minute chunks named
//   {Location}_{YYYYMMDD}-{HHMM}_{16-hex-FILETIME}.trm
// (e.g. "CR24_20180621-1449_01d4096f0757ee50.trm"). The hex suffix is the
// chunk's start time as a Windows FILETIME — fixed width, so lexicographic
// order IS chronological order, with sub-second precision and no midnight
// rollover issues. File mtimes are useless here: copying the files (how
// court audio is delivered) rewrites them.
const FTR_NAME_RE = /^(.+)_(\d{8})-(\d{4,6})_([0-9a-f]{16})\.(trm|ftr)$/i
const CHUNK_EXT_RE = /\.(trm|ftr)$/i

/**
 * Chronologically order a batch of FTR recording chunks (.trm/.ftr).
 *
 * Only reorders when the ENTIRE batch is FTR chunks — a mixed selection keeps
 * the user's order. Chunks from different recording locations (courtrooms)
 * are grouped rather than interleaved. Names that don't match the FTR pattern
 * fall back to a numeric-aware natural sort of the filename, with the
 * readable time field right-padded to HHMMSS first — numeric collation would
 * otherwise rank 1451 (14:51) before 145000 (14:50:00).
 *
 * Works on plain path strings; pass an accessor for arrays of objects.
 */
export function sortRecordingChunks(items, getPath = (x) => x) {
  if (!Array.isArray(items) || items.length < 2) return items
  if (!items.every(it => CHUNK_EXT_RE.test(getPath(it) || ''))) return items

  const natural = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' })
  // Normalize a 4-5 digit time field to 6 digits so HHMM and HHMMSS names
  // collate chronologically in the natural-sort fallback
  const padTime = (n) => n.replace(/(_\d{8}-)(\d{4,5})(?!\d)/, (_, p, t) => p + t.padEnd(6, '0'))

  return [...items].sort((x, y) => {
    const na = basename(getPath(x)), nb = basename(getPath(y))
    const ma = na.match(FTR_NAME_RE), mb = nb.match(FTR_NAME_RE)
    if (ma && mb) {
      // Same courtroom first, then chronological by the FILETIME hex
      const loc = natural(ma[1], mb[1])
      if (loc !== 0) return loc
      const ha = ma[4].toLowerCase(), hb = mb[4].toLowerCase()
      if (ha !== hb) return ha < hb ? -1 : 1
    }
    return natural(padTime(na), padTime(nb))
  })
}
