import { expect, test } from 'bun:test'
import { toggleStream } from './streamSelection'

test('selects multiple flows from one service and clears the selection when the last is removed', () => {
  const first = { flow_id: 'flow-a', service_id: 'http' }
  const second = { flow_id: 'flow-b', service_id: 'http' }
  const selection = toggleStream(null, first, true)
  expect(toggleStream(selection, second, true)).toEqual({ service_id: 'http', flow_ids: ['flow-a', 'flow-b'] })
  expect(toggleStream(selection, { flow_id: 'other', service_id: 'udp' }, true)).toEqual(selection)
  expect(toggleStream({ service_id: 'http', flow_ids: ['flow-a'] }, first, false)).toBeNull()
})

test('does not duplicate a selected flow and ignores deselection for another service', () => {
  const selection = { service_id: 'tcp', flow_ids: ['flow-a'] }
  expect(toggleStream(selection, { flow_id: 'flow-a', service_id: 'tcp' }, true)).toEqual(selection)
  expect(toggleStream(selection, { flow_id: 'other', service_id: 'udp' }, false)).toEqual(selection)
})
