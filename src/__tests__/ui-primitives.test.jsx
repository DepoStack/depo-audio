import { useState } from 'react'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Switch } from '../components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../components/ui/tabs'

function ControlledSwitch() {
  const [checked, setChecked] = useState(false)
  return <Switch aria-label="Enable cleanup" checked={checked} onCheckedChange={setChecked} />
}

describe('updated UI primitives', () => {
  it('keeps the controlled switch interaction contract', () => {
    render(<ControlledSwitch />)

    const toggle = screen.getByRole('switch', { name: 'Enable cleanup' })
    expect(toggle).toHaveAttribute('aria-checked', 'false')

    fireEvent.click(toggle)

    expect(toggle).toHaveAttribute('aria-checked', 'true')
  })

  it('keeps vertical tab keyboard focus and automatic activation', async () => {
    render(
      <Tabs defaultValue="convert" orientation="vertical">
        <TabsList aria-label="Workspace">
          <TabsTrigger value="convert">Convert</TabsTrigger>
          <TabsTrigger value="player">Player</TabsTrigger>
        </TabsList>
        <TabsContent value="convert">Convert panel</TabsContent>
        <TabsContent value="player">Player panel</TabsContent>
      </Tabs>,
    )

    const convert = screen.getByRole('tab', { name: 'Convert' })
    const player = screen.getByRole('tab', { name: 'Player' })
    convert.focus()

    fireEvent.keyDown(convert, { key: 'ArrowDown' })

    await waitFor(() => expect(player).toHaveFocus())
    expect(player).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByText('Player panel')).toBeVisible()
  })
})
