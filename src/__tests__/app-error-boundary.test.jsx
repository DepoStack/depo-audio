import { fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import AppErrorBoundary from '../components/AppErrorBoundary'

function BrokenView() {
  throw new Error('synthetic render failure')
}

describe('AppErrorBoundary', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('keeps ordinary app content unchanged', () => {
    render(
      <AppErrorBoundary>
        <p>Conversion workspace</p>
      </AppErrorBoundary>,
    )

    expect(screen.getByText('Conversion workspace')).toBeVisible()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('announces a render failure, focuses recovery, and reloads only on request', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const onReload = vi.fn()

    render(
      <AppErrorBoundary onReload={onReload}>
        <BrokenView />
      </AppErrorBoundary>,
    )

    const recovery = screen.getByRole('alert')
    expect(recovery).toHaveFocus()
    expect(screen.getByRole('heading', { name: 'DepoAudio needs to reload' })).toBeVisible()
    expect(screen.getByText('No diagnostic information is sent automatically.')).toBeVisible()
    expect(onReload).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Reload DepoAudio' }))

    expect(onReload).toHaveBeenCalledTimes(1)
  })
})
