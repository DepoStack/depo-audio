export const MODES = [
  { id: 'stereo', label: 'Mix to Stereo',  desc: 'All channels blended to one stereo file' },
  { id: 'keep',   label: 'Keep Original',  desc: 'Convert container, preserve channel layout' },
  { id: 'split',  label: 'Split Channels', desc: 'One file per channel, named by role' },
]

export const FORMATS_OUT = [
  { id: 'wav',  label: 'WAV',  desc: 'Lossless PCM — editing' },
  { id: 'mp3',  label: 'MP3',  desc: '128–320 kbps — scopists / email' },
  { id: 'flac', label: 'FLAC', desc: 'Lossless compressed — archival' },
  { id: 'opus', label: 'Opus', desc: '64 kbps VBR — voice optimized, smallest' },
  { id: 'm4a',  label: 'M4A',  desc: '128 kbps AAC — Apple devices' },
]

// Selectable MP3 bitrates (kbps), shown when MP3 output is chosen.
export const MP3_BITRATES = [
  { value: 128, label: '128k', desc: 'Smallest — email' },
  { value: 192, label: '192k', desc: 'Balanced — general use' },
  { value: 320, label: '320k', desc: 'Highest quality' },
]

export const CH_COLORS = ['#c49a36','#4a8fdf','#3a9e6a','#c94e4e']

export const FORMAT_ROWS = [
  // Standard formats — play and import directly, convert optionally
  { ext: '.wav',                    vendor: 'Standard PCM',                         ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.mp3',                    vendor: 'Standard',                             ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.flac',                   vendor: 'Standard Lossless',                    ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.m4a  .aac',              vendor: 'Apple / Standard AAC',                 ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.ogg  .opus',             vendor: 'Standard / Voice-optimized',           ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.wma  .aif  .aiff',       vendor: 'Windows Media / Apple AIFF',           ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.caf  .amr  .3ga',        vendor: 'Apple Core Audio / Phone recordings',  ch: 'any',    status: 'supported',     group: 'standard' },
  { ext: '.mp4  .mov  .mkv  .avi  .webm', vendor: 'Video — audio track extracted',  ch: 'any',    status: 'supported',     group: 'standard' },
  // Court reporting formats — require conversion
  { ext: '.sgmca',                  vendor: 'Stenograph · Case CATalyst',           ch: '4 ch',   status: 'supported',     group: 'court' },
  { ext: '.trm  .ftr',              vendor: 'For The Record · FTR Gold',            ch: '4–16 ch',status: 'experimental',  group: 'court' },
  { ext: '.bwf',                    vendor: 'CourtSmart · Various',                 ch: 'varies', status: 'supported',     group: 'court' },
  { ext: '.dm',                     vendor: 'Stenovations · DigitalCAT',            ch: '—',      status: 'experimental',  group: 'court' },
  { ext: '.aes',                    vendor: 'Eclipse CAT · AudioSync',              ch: '—',      status: 'unsupported',   group: 'court' },
  { ext: '.dcr',                    vendor: 'High Criteria · Liberty',              ch: '—',      status: 'unsupported',   group: 'court' },
]

/// Standard formats that can be played/imported without conversion
export const STANDARD_EXTS = new Set(['wav','mp3','flac','m4a','aac','ogg','opus','wma','aif','aiff','caf','amr'])
