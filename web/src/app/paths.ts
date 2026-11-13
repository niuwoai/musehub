export const DEFAULT_ORG = 'demo'

export const paths = {
  login: () => '/login',
  repos: (org: string) => `/org/${org}/repos`,
  orgSettings: (org: string) => `/org/${org}/settings`,
  members: (org: string) => `/org/${org}/members`,
  audit: (org: string) => `/org/${org}/audit`,
  runner: (org: string) => `/org/${org}/runner`,
  accountSecurity: () => '/account/security',
  accountTokens: () => '/account/tokens',
  repoIssues: (org: string, slug: string) => `/org/${org}/repos/${slug}/issues`,
  repoIssue: (org: string, slug: string, number: number) =>
    `/org/${org}/repos/${slug}/issues/${number}`,
  repoPulls: (org: string, slug: string) => `/org/${org}/repos/${slug}/pulls`,
  repoPull: (org: string, slug: string, number: number) =>
    `/org/${org}/repos/${slug}/pulls/${number}`,
  repoCode: (org: string, slug: string) => `/org/${org}/repos/${slug}/code`,
  repoDelivery: (org: string, slug: string) => `/org/${org}/repos/${slug}/delivery`,
  repoDeliveryDeepLink: (
    org: string,
    slug: string,
    query: { deployment?: string; pipeline?: string },
  ) => {
    const params = new URLSearchParams()
    if (query.deployment) params.set('deployment', query.deployment)
    if (query.pipeline) params.set('pipeline', query.pipeline)
    const q = params.toString()
    const base = `/org/${org}/repos/${slug}/delivery`
    return q ? `${base}?${q}` : base
  },
  repoSettings: (org: string, slug: string) => `/org/${org}/repos/${slug}/settings`,
} as const
