// Typed client for the Pulse API (docs/pulse-api-spec.md). Every function
// throws ApiError on a non-2xx response, using the standard error envelope
// `{ error: { code, message } }` (api-spec #7).

const API_URL = process.env.NEXT_PUBLIC_API_URL ?? "http://localhost:8080";
const TOKEN_KEY = "pulse.token";

export class ApiError extends Error {
  code: string;
  status: number;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

// localStorage isn't available during server-side rendering; every caller
// of these lives in a "use client" component, but guard anyway.
export function getToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(token: string | null) {
  if (typeof window === "undefined") return;
  try {
    if (token) window.localStorage.setItem(TOKEN_KEY, token);
    else window.localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Private browsing / storage disabled: session just won't persist.
  }
}

type Query = Record<string, string | number | boolean | undefined>;

async function request<T>(
  method: string,
  path: string,
  opts: { body?: unknown; query?: Query; auth?: boolean } = {},
): Promise<T> {
  const { body, query, auth = true } = opts;
  const url = new URL(API_URL + path);
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined) url.searchParams.set(key, String(value));
    }
  }

  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (auth) {
    const token = getToken();
    if (token) headers["Authorization"] = `Bearer ${token}`;
  }

  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new ApiError(0, "NETWORK_ERROR", "Could not reach the Pulse API. Is it running?");
  }

  if (res.status === 204) return undefined as T;

  const text = await res.text();
  const json = text ? safeJsonParse(text) : null;

  if (!res.ok) {
    const code = json?.error?.code ?? "UNKNOWN_ERROR";
    const message = json?.error?.message ?? `Request failed with status ${res.status}`;
    if (res.status === 401 && auth) setToken(null); // stale/expired token
    throw new ApiError(res.status, code, message);
  }
  return json as T;
}

function safeJsonParse(text: string): { error?: { code?: string; message?: string } } | null {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

// ---------- auth (api-spec #1) ----------

export interface CurrentUser {
  id: string;
  email: string;
  role: "admin" | "user";
  created_at: string;
}

export function register(email: string, password: string) {
  return request<{ user_id: string; email: string }>("POST", "/api/v1/auth/register", {
    body: { email, password },
    auth: false,
  });
}

export function login(email: string, password: string) {
  return request<{ token: string; user: { id: string; email: string; role: string } }>(
    "POST",
    "/api/v1/auth/login",
    { body: { email, password }, auth: false },
  );
}

export function me() {
  return request<CurrentUser>("GET", "/api/v1/auth/me");
}

export function logout() {
  return request<void>("POST", "/api/v1/auth/logout");
}

// ---------- endpoints (api-spec #2) ----------

export interface Endpoint {
  id: string;
  name: string;
  url: string;
  method: string;
  check_interval_seconds: number;
  latency_threshold_ms: number;
  error_rate_threshold_percent: number;
  is_active: boolean;
  last_checked_at: string | null;
  created_at: string;
}

export interface EndpointInput {
  name: string;
  url: string;
  method?: string;
  check_interval_seconds: number;
  latency_threshold_ms: number;
  error_rate_threshold_percent: number;
}

export function listEndpoints() {
  return request<{ endpoints: Endpoint[] }>("GET", "/api/v1/endpoints");
}

export function getEndpoint(id: string) {
  return request<Endpoint>("GET", `/api/v1/endpoints/${id}`);
}

export function createEndpoint(input: EndpointInput) {
  return request<Endpoint>("POST", "/api/v1/endpoints", { body: input });
}

export function updateEndpoint(id: string, input: Partial<EndpointInput & { is_active: boolean }>) {
  return request<Endpoint>("PATCH", `/api/v1/endpoints/${id}`, { body: input });
}

export function deleteEndpoint(id: string) {
  return request<void>("DELETE", `/api/v1/endpoints/${id}`);
}

// ---------- metrics (api-spec #3) ----------

export interface Check {
  checked_at: string;
  status_code: number | null;
  latency_ms: number | null;
  success: boolean;
  error_message: string | null;
}

export interface ChecksSummary {
  avg_latency_ms: number | null;
  success_rate_percent: number;
  total_checks: number;
}

export interface HealthDigest {
  period_start: string;
  period_end: string;
  total_checks: number;
  success_count: number;
  avg_latency_ms: number | null;
  status: "healthy" | "degraded";
}

export function listChecks(endpointId: string, opts: { limit?: number } = {}) {
  return request<{ checks: Check[] }>("GET", `/api/v1/endpoints/${endpointId}/checks`, {
    query: { limit: opts.limit },
  });
}

export function checksSummary(endpointId: string, period: "24h" | "7d" | "30d" = "24h") {
  return request<ChecksSummary>("GET", `/api/v1/endpoints/${endpointId}/checks/summary`, {
    query: { period },
  });
}

export function listHealthDigests(endpointId: string, opts: { limit?: number } = {}) {
  return request<{ digests: HealthDigest[] }>("GET", `/api/v1/endpoints/${endpointId}/health-digests`, {
    query: { limit: opts.limit },
  });
}

// ---------- incidents (api-spec #4) ----------

export interface IncidentSummary {
  id: string;
  endpoint_id: string;
  endpoint_name: string;
  trigger_reason: string;
  triggered_at: string;
  recovered_at: string | null;
  resolved_at: string | null;
  ai_status: "pending" | "completed" | "failed";
  ai_possible_cause: string | null;
  ai_confidence: "high" | "medium" | "low" | null;
}

export interface IncidentDetail extends IncidentSummary {
  metric_before: MetricSnapshot | null;
  metric_after: MetricSnapshot | null;
  ai_evidence: string[] | null;
  ai_suggested_steps: string[] | null;
  ai_error: string | null;
  created_at: string;
}

export interface MetricSnapshot {
  period: string;
  window_start: string | null;
  window_end: string | null;
  total_checks: number;
  failed_checks: number;
  avg_latency_ms: number | null;
  error_rate_percent: number;
}

export function listIncidents(opts: { endpointId?: string; status?: "open" | "resolved" } = {}) {
  return request<{ incidents: IncidentSummary[] }>("GET", "/api/v1/incidents", {
    query: { endpoint_id: opts.endpointId, status: opts.status },
  });
}

export function getIncident(id: string) {
  return request<IncidentDetail>("GET", `/api/v1/incidents/${id}`);
}

export function resolveIncident(id: string) {
  return request<IncidentDetail>("PATCH", `/api/v1/incidents/${id}/resolve`);
}
