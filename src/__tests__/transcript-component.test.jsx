import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import Transcript from '../components/Player/Transcript'
import { storageKey, syncKey } from '../lib/transcript'

const dialogMocks = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }))
const fileMocks = vi.hoisted(() => ({ readTextFile: vi.fn(), writeTextFile: vi.fn() }))

vi.mock('@tauri-apps/plugin-dialog', () => dialogMocks)
vi.mock('@tauri-apps/plugin-fs', () => fileMocks)

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  localStorage.clear()
  dialogMocks.open.mockReset()
  dialogMocks.save.mockReset()
  fileMocks.readTextFile.mockReset()
  fileMocks.writeTextFile.mockReset()
})

const PATH = 'C:\\audio\\synthetic-hearing.wav'
const SRT = `1
00:00:01,000 --> 00:00:03,000
Q: Good morning.

2
00:00:05,000 --> 00:00:07,000
A: Morning.

3
00:00:10,000 --> 00:00:12,000
Q: Please state your name.
`
const TOD = `09:00:10  Q.  First question.
09:00:20  A.  First answer.`

function renderPanel(props = {}) {
  const onPlayFrom = vi.fn()
  const utils = render(
    <Transcript
      trackPath={PATH}
      currentTime={0}
      playing={false}
      duration={600}
      onSeek={vi.fn()}
      onPlayFrom={onPlayFrom}
      {...props}
    />,
  )
  return { ...utils, onPlayFrom }
}

async function importText(text, name = 'C:\\transcripts\\day1.srt') {
  dialogMocks.open.mockResolvedValue(name)
  fileMocks.readTextFile.mockResolvedValue(text)
  fireEvent.click(screen.getByRole('button', { name: /open transcript|open another/i }))
  await waitFor(() => expect(fileMocks.readTextFile).toHaveBeenCalled())
}

const lines = () => screen.getAllByRole('option')

describe('Transcript proofing', () => {
  it('invites the person to proof an existing transcript', () => {
    renderPanel()
    expect(screen.getByText('Proof a transcript against this recording')).toBeVisible()
  })

  it('imports SRT and says what it found', async () => {
    renderPanel()
    await importText(SRT)
    expect(await screen.findByText(/day1\.srt: 3 timed lines; times look like time into the recording/)).toBeVisible()
    expect(lines()).toHaveLength(3)
    expect(JSON.parse(localStorage.getItem(storageKey(PATH)))).toHaveLength(3)
  })

  it('never offers editable transcript text', async () => {
    renderPanel()
    await importText(SRT)
    await screen.findAllByRole('option')
    const list = screen.getByRole('listbox')
    expect(within(list).queryAllByRole('textbox')).toHaveLength(0)
    expect(document.querySelectorAll('textarea, [contenteditable="true"]')).toHaveLength(0)
    expect(screen.queryByRole('button', { name: /add line|start typing|stamp|export srt/i })).not.toBeInTheDocument()
  })

  it('shows markup-looking cue text as plain text, never as HTML', async () => {
    renderPanel()
    await importText(
      '1\n00:00:01,000 --> 00:00:02,000\n<scr<script>ipt>alert(1)</script> <img src=x onerror=alert(1)>\n',
    )
    const list = await screen.findByRole('listbox')
    expect(within(list).getByText(/<scr<script>ipt>alert\(1\)<\/script> <img src=x onerror=alert\(1\)>/)).toBeVisible()
    expect(document.querySelectorAll('script, img')).toHaveLength(0)
  })

  it('highlights the line at the playhead, with the sync shift applied', async () => {
    const { rerender } = renderPanel({ currentTime: 6 })
    await importText(SRT)
    await waitFor(() => expect(lines()[1]).toHaveAttribute('aria-current', 'true'))
    fireEvent.click(screen.getByRole('button', { name: 'Move lines 0.5 seconds later' }))
    fireEvent.click(screen.getByRole('button', { name: 'Move lines 0.5 seconds later' }))
    expect(screen.getByText('Transcript is ahead by 1.0 s')).toBeVisible()
    expect(JSON.parse(localStorage.getItem(syncKey(PATH))).shift).toBe(1)
    rerender(<Transcript trackPath={PATH} currentTime={5.5} playing duration={600} onSeek={vi.fn()} />)
    // Line 2 (00:00:05) now plays from 6.0 s, so line 1 is still current.
    expect(lines()[0]).toHaveAttribute('aria-current', 'true')
  })

  it('plays from a line on click or Enter, and moves with the arrow keys', async () => {
    const { onPlayFrom } = renderPanel()
    await importText(SRT)
    fireEvent.click((await screen.findAllByRole('option'))[2])
    expect(onPlayFrom).toHaveBeenLastCalledWith(10)
    lines()[0].focus()
    fireEvent.keyDown(lines()[0], { key: 'ArrowDown' })
    expect(lines()[1]).toHaveFocus()
    fireEvent.keyDown(lines()[1], { key: 'Enter' })
    expect(onPlayFrom).toHaveBeenLastCalledWith(5)
    fireEvent.keyDown(lines()[1], { key: 'End' })
    expect(lines()[2]).toHaveFocus()
  })

  it('"Set this line to now" shifts the transcript and says so', async () => {
    renderPanel({ currentTime: 12.5 })
    await importText(SRT)
    fireEvent.focus((await screen.findAllByRole('option'))[2])
    fireEvent.click(screen.getByRole('button', { name: 'Set row 3 to now' }))
    expect(screen.getByText('Transcript is ahead by 2.5 s')).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Set row 3 to play from 0:00:12.')
    fireEvent.click(screen.getByRole('button', { name: 'Clear shift' }))
    expect(screen.getByText('No sync shift')).toBeVisible()
  })

  it('flags a line with F, keeps a note, and never changes the text', async () => {
    renderPanel()
    await importText(SRT)
    const before = JSON.parse(localStorage.getItem(storageKey(PATH)) ?? '[]')
    fireEvent.keyDown((await screen.findAllByRole('option'))[1], { key: 'f' })
    const note = screen.getByRole('textbox', { name: 'Note for flag on row 2' })
    fireEvent.change(note, { target: { value: 'speaker?' } })
    const after = JSON.parse(localStorage.getItem(storageKey(PATH)))
    expect(after[1].flag).toEqual({ note: 'speaker?' })
    expect(after.map(({ start, speaker, text }) => ({ start, speaker, text }))).toEqual(
      before.map(({ start, speaker, text }) => ({ start, speaker, text })),
    )
    expect(screen.getByText('Flags for review (1)')).toBeVisible()
  })

  it('exports flags as CSV through the save dialog', async () => {
    renderPanel()
    await importText(SRT)
    fireEvent.keyDown((await screen.findAllByRole('option'))[0], { key: 'F' })
    dialogMocks.save.mockResolvedValue('C:\\out\\flags.csv')
    fileMocks.writeTextFile.mockResolvedValue()
    fireEvent.click(screen.getByRole('button', { name: /export flags/i }))
    await waitFor(() => expect(fileMocks.writeTextFile).toHaveBeenCalled())
    const [path, content] = fileMocks.writeTextFile.mock.calls[0]
    expect(path).toBe('C:\\out\\flags.csv')
    expect(content).toMatch(/^Page,Line,Row,Audio time,Time of day,Excerpt,Note\r\n,,1,0:00:01,,Q: Good morning\.,/)
    expect(dialogMocks.save.mock.calls[0][0].defaultPath).toBe('synthetic-hearing proofing flags.csv')
  })

  it('asks for the recording start for time-of-day stamps, then maps lines to the audio', async () => {
    const { onPlayFrom } = renderPanel()
    await importText(TOD, 'C:\\transcripts\\day1.txt')
    expect(await screen.findByText(/2 timed lines; times look like time of day/)).toBeVisible()
    fireEvent.click(lines()[1])
    expect(onPlayFrom).not.toHaveBeenCalled()
    expect(screen.getByText(/does not know when this recording started/)).toBeVisible()

    const field = screen.getByLabelText('Recording started at')
    fireEvent.change(field, { target: { value: 'nine' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(screen.getByText(/Type the start time/)).toBeVisible()

    fireEvent.change(field, { target: { value: '9:00:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    fireEvent.click(lines()[1])
    expect(onPlayFrom).toHaveBeenLastCalledWith(20)
    expect(JSON.parse(localStorage.getItem(syncKey(PATH)))).toMatchObject({
      timeKind: 'time-of-day',
      recordingStart: 9 * 3600,
    })
  })

  it('lets the person say the times are elapsed instead', async () => {
    renderPanel()
    await importText(TOD, 'C:\\transcripts\\day1.txt')
    fireEvent.click(await screen.findByRole('button', { name: 'time into recording' }))
    expect(screen.queryByLabelText('Recording started at')).not.toBeInTheDocument()
    expect(lines()[0]).toHaveTextContent('9:00:10')
  })

  it('F flags the line being heard: the focus follows the playing line, and the flag is shown', async () => {
    const { rerender } = renderPanel()
    await importText(SRT)
    lines()[0].focus()
    const props = { trackPath: PATH, duration: 600, onSeek: vi.fn(), onPlayFrom: vi.fn() }
    rerender(<Transcript {...props} currentTime={0.5} playing />)
    rerender(<Transcript {...props} currentTime={10.5} playing />)
    expect(lines()[2]).toHaveAttribute('aria-current', 'true')
    expect(lines()[2]).toHaveFocus()
    fireEvent.keyDown(lines()[2], { key: 'f' })
    expect(screen.getByRole('status')).toHaveTextContent('Flagged row 3 for review.')
    expect(screen.getByRole('status')).toBeVisible()
    expect(JSON.parse(localStorage.getItem(storageKey(PATH)))[2].flag).toEqual({ note: '' })
  })

  it('pauses following while the person moves through the lines, and goes back with "."', async () => {
    const { rerender } = renderPanel()
    await importText(SRT)
    const props = { trackPath: PATH, duration: 600, onSeek: vi.fn(), onPlayFrom: vi.fn() }
    rerender(<Transcript {...props} currentTime={0.5} playing />)
    lines()[0].focus()
    fireEvent.keyDown(lines()[0], { key: 'ArrowDown' })
    rerender(<Transcript {...props} currentTime={10.5} playing />)
    // The focus stays where the person put it.
    expect(lines()[1]).toHaveFocus()
    expect(screen.getByRole('button', { name: /back to playing line/i })).toHaveAttribute('aria-pressed', 'false')
    fireEvent.keyDown(lines()[1], { key: '.' })
    expect(lines()[2]).toHaveFocus()
    expect(screen.getByRole('button', { name: 'Follow audio' })).toHaveAttribute('aria-pressed', 'true')
  })

  it('reads the flag as part of the line, with no button inside the line', async () => {
    renderPanel()
    await importText(SRT)
    fireEvent.keyDown((await screen.findAllByRole('option'))[1], { key: 'f' })
    expect(within(screen.getByRole('listbox')).queryAllByRole('button')).toHaveLength(0)
    expect(lines()[1]).toHaveAccessibleName(/^row 2:.*Morning\., flagged$/)
    expect(lines()[1]).not.toHaveAttribute('aria-selected')
    expect(screen.getByRole('button', { name: 'Play from row 2, 0:00:05' })).toBeVisible()
  })

  it('names lines by page and line from a CAT file', async () => {
    renderPanel()
    await importText('                  4\n  7  00:00:05  Q.  On page four.', 'C:\\transcripts\\day1.txt')
    expect(await screen.findByRole('option', { name: /^page 4, line 7:/ })).toBeVisible()
    expect(screen.getByText('4:7')).toBeVisible()
    expect(lines()).toHaveLength(1)
  })

  it('says when a time-of-day line cannot be placed, instead of playing from the start', async () => {
    const { onPlayFrom } = renderPanel()
    await importText('09:00:10  Q.  Morning.\n21:30:00  A.  Evening.', 'C:\\transcripts\\day1.txt')
    fireEvent.change(await screen.findByLabelText('Recording started at'), { target: { value: '9:00:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    fireEvent.click(lines()[1])
    expect(onPlayFrom).not.toHaveBeenCalled()
    expect(screen.getByRole('status')).toHaveTextContent(/outside the recording/)
  })

  it('copies the lines with their times', async () => {
    const writeText = vi.fn().mockResolvedValue()
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    renderPanel()
    await importText(SRT)
    fireEvent.click(await screen.findByRole('button', { name: /copy with times/i }))
    await waitFor(() => expect(writeText).toHaveBeenCalled())
    expect(writeText.mock.calls[0][0].split('\n')[0]).toBe('0:00:01.000\tQ: Good morning.')
  })

  it('reads a transcript saved by an earlier version as time into the recording, whatever its first time', () => {
    localStorage.setItem(
      storageKey(PATH),
      JSON.stringify([{ id: 'a', start: 10 * 3600, speaker: '', text: 'Broadcast timecode' }]),
    )
    const { onPlayFrom } = renderPanel()
    expect(screen.getByRole('button', { name: 'time into recording' })).toHaveAttribute('aria-pressed', 'true')
    expect(screen.queryByLabelText('Recording started at')).not.toBeInTheDocument()
    fireEvent.click(lines()[0])
    expect(onPlayFrom).toHaveBeenLastCalledWith(10 * 3600)
  })

  it('opens a transcript saved by an earlier version, read-only', () => {
    localStorage.setItem(
      storageKey(PATH),
      JSON.stringify([
        { id: 'a', start: 1, speaker: 'Q', text: 'Typed in an earlier version' },
        { id: 'b', start: null, speaker: '', text: 'Never stamped' },
      ]),
    )
    renderPanel()
    expect(screen.getByText(/Saved transcript: 1 timed line, 1 without a time/)).toBeVisible()
    expect(lines()[0]).toHaveTextContent('Typed in an earlier version')
    expect(lines()[1]).toHaveTextContent('Never stamped')
    expect(document.querySelectorAll('textarea')).toHaveLength(0)
    // Opening it writes nothing back.
    expect(JSON.parse(localStorage.getItem(storageKey(PATH)))[0]).toEqual({
      id: 'a',
      start: 1,
      speaker: 'Q',
      text: 'Typed in an earlier version',
    })
  })

  it('asks before replacing a stored transcript with another file', async () => {
    localStorage.setItem(storageKey(PATH), JSON.stringify([{ id: 'a', start: 1, speaker: '', text: 'Keep me' }]))
    renderPanel()
    await importText(SRT)
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent(/keeps 1 line for this track/)
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(JSON.parse(localStorage.getItem(storageKey(PATH)))[0].text).toBe('Keep me')
  })

  it('reports an import that cannot be read, and an empty file', async () => {
    renderPanel()
    dialogMocks.open.mockResolvedValue('C:\\transcripts\\bad.txt')
    fileMocks.readTextFile.mockRejectedValue(new Error('permission denied'))
    fireEvent.click(screen.getByRole('button', { name: /open transcript/i }))
    expect(await screen.findByRole('alert')).toHaveTextContent('The transcript could not be opened')

    fileMocks.readTextFile.mockResolvedValue('\n\n')
    fireEvent.click(screen.getByRole('button', { name: /open transcript/i }))
    expect(await screen.findByText(/No transcript lines were found in bad\.txt/)).toBeVisible()
  })

  it('warns when saving is unavailable, and keeps the flags on screen', async () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('storage blocked')
    })
    renderPanel()
    await importText(SRT)
    expect(await screen.findByRole('alert')).toHaveTextContent('Saving is unavailable on this computer')
    fireEvent.keyDown(lines()[0], { key: 'f' })
    expect(screen.getByText('Flags for review (1)')).toBeVisible()
  })

  it('removes only the stored copy for this track, after confirmation', () => {
    const other = 'C:\\audio\\other.wav'
    localStorage.setItem(storageKey(PATH), JSON.stringify([{ id: 'a', start: 1, speaker: 'Q', text: 'Line' }]))
    localStorage.setItem(syncKey(PATH), JSON.stringify({ shift: 1 }))
    localStorage.setItem(storageKey(other), JSON.stringify([{ id: 'b', start: 2, speaker: 'A', text: 'Keep me' }]))
    renderPanel()

    fireEvent.click(screen.getByRole('button', { name: 'Remove this transcript from DepoAudio' }))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(localStorage.getItem(storageKey(PATH))).not.toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Remove this transcript from DepoAudio' }))
    expect(screen.getByRole('dialog')).toHaveTextContent(
      /The transcript file you opened and the recording are not affected/,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Remove transcript' }))

    expect(localStorage.getItem(storageKey(PATH))).toBeNull()
    expect(localStorage.getItem(syncKey(PATH))).toBeNull()
    expect(JSON.parse(localStorage.getItem(storageKey(other)))[0].text).toBe('Keep me')
    expect(screen.getByText('Proof a transcript against this recording')).toBeVisible()
    expect(fileMocks.writeTextFile).not.toHaveBeenCalled()
  })
})
