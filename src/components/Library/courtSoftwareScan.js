// Keep selected scans separate from the last successful, source-attributed jobs.
export const initialCourtSoftwareScan = {
  requestId: 0,
  phase: 'idle',
  software: null,
  selected: null,
  resultSource: null,
  jobs: [],
  error: '',
}

export function courtSoftwareScanReducer(state, action) {
  if (action.type === 'dismiss') return { ...initialCourtSoftwareScan, requestId: action.requestId }
  if (action.type === 'detect') {
    return { ...state, requestId: action.requestId, phase: 'detecting', selected: null, error: '' }
  }
  if (action.type === 'select') {
    return { ...state, requestId: action.requestId, phase: 'scanning', selected: action.source, error: '' }
  }
  // Superseded requests must not replace current selection, results or errors.
  if (action.requestId !== state.requestId) return state
  if (action.type === 'detected') {
    return {
      ...state,
      software: action.software,
      phase: 'ready',
      ...(action.software.length === 0 ? { jobs: [], resultSource: null } : {}),
    }
  }
  if (action.type === 'complete') {
    return { ...state, phase: 'ready', jobs: action.jobs, resultSource: state.selected, error: '' }
  }
  if (action.type === 'failed') return { ...state, phase: 'error', error: action.error }
  return state
}
