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
  repoSettings: (org: string, slug: string) => `/org/${org}/repos/${slug}/settings`,
} as const
