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
