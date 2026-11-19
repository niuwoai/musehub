import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { IssueDetailPage } from './IssueDetailPage'
import '../i18n'

const api = vi.hoisted(() => ({
  acceptPlan: vi.fn(),
  cancelAgentRun: vi.fn(),
  createPlan: vi.fn(),
  getAgentRun: vi.fn(),
  getIssue: vi.fn(),
  getIssueDeliveryTrace: vi.fn(),
  listIssueAssignees: vi.fn(),
  listIssueComments: vi.fn(),
  patchIssue: vi.fn(),
  postIssueComment: vi.fn(),
  replaceIssueCriteria: vi.fn(),
  respondAgentRun: vi.fn(),
  startAgentRun: vi.fn(),
}))

vi.mock('../api', () => api)

const issue = {
  id: 'issue-1',
  number: 1,
  title: 'Keyboard plan acceptance',
  state: 'open',
  body: 'Issue body',
  author_handle: 'admin',
  revision: 1,
  current_revision_id: 'revision-1',
  criteria: [],
}

function renderIssue() {
  return render(
    <MemoryRouter initialEntries={['/org/demo/repos/example/issues/1']}>
      <Routes>
        <Route path="/org/:org/repos/:slug/issues/:number" element={<IssueDetailPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

describe('IssueDetailPage keyboard focus', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.getIssue
      .mockResolvedValueOnce(issue)
      .mockResolvedValue({
        ...issue,
        plan: { id: 'plan-1', version: 1, state: 'accepted', body: 'A keyboard-friendly plan' },
      })
    api.listIssueAssignees.mockResolvedValue([])
    api.listIssueComments.mockResolvedValue([])
    api.getIssueDeliveryTrace.mockResolvedValue(null)
    api.createPlan.mockResolvedValue({})
    api.acceptPlan.mockResolvedValue({})
  })

  afterEach(() => cleanup())

  it('moves focus to the enabled AgentRun action after plan acceptance', async () => {
    renderIssue()

    const planInput = await screen.findByTestId('issue-plan-input')
    const acceptPlan = screen.getByTestId('issue-accept-plan')
    const startAgent = screen.getByTestId('issue-run-agent')
    fireEvent.change(planInput, { target: { value: 'A keyboard-friendly plan' } })
    expect((startAgent as HTMLButtonElement).disabled).toBe(true)
    acceptPlan.focus()

    fireEvent.click(acceptPlan)

    await waitFor(() => expect((startAgent as HTMLButtonElement).disabled).toBe(false))
    expect(document.activeElement).toBe(startAgent)
  })
})
