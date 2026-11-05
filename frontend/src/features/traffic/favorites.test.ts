import { expect, test } from 'bun:test'
import { canReadSavedHistory, flowRows } from './traffic'
import { updateFlows } from './live'

const opening = { kind: 'flow_opened', flow_id: 'one', service_id: 'tcp', transport: 1 }
test('traffic rows expose durable favourite state and incomplete capture details', () => {
  const [row] = flowRows({ items: [{ ...opening, favorite: true, raw_history_state: 'unavailable', decoded_incomplete: true }], next_cursor: null })
  expect(row?.favorite).toBe(true)
  expect(row?.raw_history_state).toBe('unavailable')
  expect(row?.decoded_incomplete).toBe(true)
  expect(flowRows({ items: [opening], next_cursor: null })[0]?.favorite).toBe(false)
})

test('replayed opening and closure events preserve a locally saved favourite', () => {
  const saved = flowRows({ items: [{ ...opening, favorite: true, raw_history_state: 'pending' }], next_cursor: null })
  const replayed = updateFlows(saved, opening)
  expect(replayed[0]?.favorite).toBe(true)
  expect(replayed[0]?.raw_history_state).toBe('pending')
  expect(updateFlows(replayed, { ...opening, kind: 'flow_closed', bytes_client_to_server: 5 })[0]?.favorite).toBe(true)
  expect(updateFlows(replayed, { ...opening, favorite: false })[0]?.favorite).toBe(false)
})

test('remote unpin events remove the favourite state before filtered reconciliation', () => {
  const saved=flowRows({items:[{...opening,favorite:true}],next_cursor:null})
  const changed=updateFlows(saved,{...opening,kind:'favorite_changed',favorite:false,raw_history_state:'removed'})
  expect(changed[0]?.favorite).toBe(false)
  expect(changed.filter(flow=>flow.favorite)).toHaveLength(0)
})

test('interrupted favourite snapshots remain readable even without a flow-close event', () => {
  expect(canReadSavedHistory({state:'Open',favorite:true,raw_history_state:'partial'})).toBe(true)
  expect(canReadSavedHistory({state:'Open',favorite:true,raw_history_state:'ready'})).toBe(true)
  expect(canReadSavedHistory({state:'Open',favorite:true,raw_history_state:'pending'})).toBe(false)
  expect(canReadSavedHistory({state:'Open',favorite:false,raw_history_state:'partial'})).toBe(false)
})
