import { FormEvent, useCallback, useEffect, useState } from 'react'
import { useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Panel } from '../components/Panel'
import {
  createWebhook,
  disableWebhook,
  listWebhookDeliveries,
  listWebhooks,
  type WebhookDelivery,
  type WebhookSubscription,
} from '../api'
import { DEFAULT_ORG } from '../app/paths'

const WEBHOOK_EVENTS = [
  'org.member.invite',
  'issue.created',
  'pull_request.created',
  'pipeline.queued',
  'deployment.start',
  'deployment.approve',
  'deployment.reject',
  'deployment.rollback',
] as const

export function OrgWebhooksPage() {
  const { t } = useTranslation()
  const { org = DEFAULT_ORG } = useParams<{ org: string }>()
  const [subscriptions, setSubscriptions] = useState<WebhookSubscription[]>([])
  const [deliveries, setDeliveries] = useState<WebhookDelivery[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [endpointUrl, setEndpointUrl] = useState('')
  const [eventTypes, setEventTypes] = useState<string[]>(['issue.created', 'pull_request.created'])
  const [secret, setSecret] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const rows = await listWebhooks(org)
      setSubscriptions(rows)
      setSelectedId((current) => current && rows.some((row) => row.id === current) ? current : null)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t('webhooks.loadFailed'))
    } finally {
      setLoading(false)
    }
  }, [org, t])

  useEffect(() => {
    setLoading(true)
    setSubscriptions([])
    setDeliveries([])
    setSecret(null)
    void refresh()
  }, [refresh])

  useEffect(() => {
    if (!selectedId) {
      setDeliveries([])
      return
    }
    let current = true
    void listWebhookDeliveries(org, selectedId)
      .then((rows) => { if (current) setDeliveries(rows) })
      .catch((reason: unknown) => {
        if (current) setError(reason instanceof Error ? reason.message : t('webhooks.deliveriesFailed'))
      })
    return () => { current = false }
  }, [org, selectedId, t])

  async function submit(event: FormEvent) {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const result = await createWebhook(org, { endpoint_url: endpointUrl.trim(), event_types: eventTypes })
      setSecret(result.secret)
      setEndpointUrl('')
      await refresh()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t('webhooks.createFailed'))
    } finally {
      setBusy(false)
    }
  }

  async function disable(id: string) {
    setBusy(true)
    setError(null)
    try {
      await disableWebhook(org, id)
      setSelectedId((selected) => selected === id ? null : selected)
      await refresh()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t('webhooks.disableFailed'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel data-testid="org-webhooks-page">
      <h2>{t('webhooks.title')}</h2>
      <p>{t('webhooks.description')}</p>
      {error ? <p role="alert">{error}</p> : null}
      {secret ? (
        <section role="status" data-testid="webhook-created-secret">
          <strong>{t('webhooks.secretOnce')}</strong>
          <pre>{secret}</pre>
          <button type="button" onClick={() => setSecret(null)}>{t('webhooks.dismissSecret')}</button>
        </section>
      ) : null}
      <form onSubmit={(event) => void submit(event)}>
        <label>
          {t('webhooks.endpoint')}
          <input
            data-testid="webhook-endpoint"
            type="url"
            required
            value={endpointUrl}
            onChange={(event) => setEndpointUrl(event.target.value)}
          />
        </label>
        <fieldset>
          <legend>{t('webhooks.events')}</legend>
          {WEBHOOK_EVENTS.map((eventType) => (
            <label key={eventType}>
              <input
                type="checkbox"
                checked={eventTypes.includes(eventType)}
                onChange={(event) => setEventTypes((current) => event.target.checked
                  ? [...current, eventType]
                  : current.filter((value) => value !== eventType))}
              />
              {eventType}
            </label>
          ))}
        </fieldset>
        <button type="submit" data-testid="webhook-create" disabled={busy || eventTypes.length === 0}>
          {busy ? t('common.loading') : t('webhooks.create')}
        </button>
      </form>
      <section>
        <h3>{t('webhooks.subscriptions')}</h3>
        {loading ? <p role="status">{t('common.loading')}</p> : null}
        {!loading && subscriptions.length === 0 ? <p>{t('webhooks.empty')}</p> : null}
        <ul data-testid="webhook-list">
          {subscriptions.map((subscription) => (
            <li key={subscription.id} data-testid={`webhook-row-${subscription.id}`}>
              <button type="button" onClick={() => setSelectedId(subscription.id)}>
                {subscription.endpoint_url} · {subscription.enabled ? t('webhooks.enabled') : t('webhooks.disabled')}
              </button>
              {subscription.enabled ? (
                <button type="button" disabled={busy} onClick={() => void disable(subscription.id)}>
                  {t('webhooks.disable')}
                </button>
              ) : null}
              {selectedId === subscription.id ? (
                <ul data-testid="webhook-deliveries">
                  {deliveries.length === 0 ? <li>{t('webhooks.deliveriesEmpty')}</li> : null}
                  {deliveries.map((delivery) => (
                    <li key={delivery.id} data-testid={`webhook-delivery-${delivery.id}`}>
                      {delivery.status} · {delivery.attempt_count} · {delivery.response_status ?? '—'}
                      {delivery.last_error ? ` · ${delivery.last_error}` : ''}
                    </li>
                  ))}
                </ul>
              ) : null}
            </li>
          ))}
        </ul>
      </section>
    </Panel>
  )
}
