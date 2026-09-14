"use client";

import { use, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { RequireAuth } from "@/lib/auth";
import {
  ApiError,
  checksSummary,
  deleteEndpoint,
  getEndpoint,
  listChecks,
  listHealthDigests,
  listIncidents,
  updateEndpoint,
} from "@/lib/api";
import { useAsync } from "@/lib/useAsync";
import { formatDateTime, relativeTime, triggerReasonLabel } from "@/lib/format";
import { EndpointForm } from "@/components/EndpointForm";
import { IncidentStatusBadge } from "@/components/IncidentStatusBadge";
import { LatencyChart } from "@/components/LatencyChart";
import { Badge, Button, Card, EmptyState, ErrorBanner, Spinner } from "@/components/ui";

export default function EndpointDetailPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  return (
    <RequireAuth>
      <EndpointDetail id={id} />
    </RequireAuth>
  );
}

function EndpointDetail({ id }: { id: string }) {
  const router = useRouter();
  const [editing, setEditing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const endpoint = useAsync(() => getEndpoint(id), [id]);
  const checks = useAsync(() => listChecks(id, { limit: 200 }), [id]);
  const summary = useAsync(() => checksSummary(id, "24h"), [id]);
  const incidents = useAsync(() => listIncidents({ endpointId: id }), [id]);
  const digests = useAsync(() => listHealthDigests(id, { limit: 10 }), [id]);

  if (endpoint.loading) return <Spinner />;
  if (endpoint.error || !endpoint.data) {
    return <ErrorBanner message={endpoint.error ?? "Endpoint not found."} />;
  }
  const ep = endpoint.data;

  async function togglePause() {
    setActionError(null);
    try {
      await updateEndpoint(id, { is_active: !ep.is_active });
      endpoint.reload();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Something went wrong.");
    }
  }

  async function handleDelete() {
    if (!confirm(`Delete "${ep.name}"? This also deletes its check history and incidents.`)) return;
    setActionError(null);
    try {
      await deleteEndpoint(id);
      router.push("/");
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Something went wrong.");
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <h1 className="text-xl font-semibold tracking-tight">{ep.name}</h1>
            {!ep.is_active && <Badge>paused</Badge>}
          </div>
          <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">
            {ep.method} {ep.url}
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="secondary" onClick={() => setEditing((v) => !v)}>
            {editing ? "Cancel" : "Edit"}
          </Button>
          <Button variant="secondary" onClick={togglePause}>
            {ep.is_active ? "Pause" : "Resume"}
          </Button>
          <Button variant="danger" onClick={handleDelete}>
            Delete
          </Button>
        </div>
      </div>

      <ErrorBanner message={actionError} />

      {editing && (
        <Card>
          <EndpointForm
            initial={ep}
            submitLabel="Save changes"
            onSubmit={async (input) => {
              await updateEndpoint(id, input);
              setEditing(false);
              endpoint.reload();
            }}
          />
        </Card>
      )}

      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <StatTile label="Success rate (24h)" value={summary.data ? `${summary.data.success_rate_percent}%` : "—"} />
        <StatTile
          label="Avg latency (24h)"
          value={summary.data?.avg_latency_ms != null ? `${Math.round(summary.data.avg_latency_ms)} ms` : "—"}
        />
        <StatTile label="Checks (24h)" value={summary.data ? String(summary.data.total_checks) : "—"} />
        <StatTile label="Last checked" value={relativeTime(ep.last_checked_at)} />
      </div>

      <Card>
        <h2 className="mb-3 text-sm font-medium text-zinc-700 dark:text-zinc-300">Latency, most recent checks</h2>
        {checks.loading ? <Spinner /> : <LatencyChart checks={checks.data?.checks ?? []} />}
      </Card>

      <Card>
        <h2 className="mb-3 text-sm font-medium text-zinc-700 dark:text-zinc-300">Incidents</h2>
        {incidents.loading && <Spinner />}
        {incidents.data && incidents.data.incidents.length === 0 && (
          <EmptyState>No incidents for this endpoint.</EmptyState>
        )}
        {incidents.data && incidents.data.incidents.length > 0 && (
          <ul className="divide-y divide-zinc-200 dark:divide-zinc-800">
            {incidents.data.incidents.map((inc) => (
              <li key={inc.id}>
                <Link
                  href={`/incidents/${inc.id}`}
                  className="flex items-center justify-between gap-4 py-3 hover:text-zinc-950 dark:hover:text-zinc-50"
                >
                  <div>
                    <div className="font-medium">{triggerReasonLabel(inc.trigger_reason)}</div>
                    <div className="text-xs text-zinc-500 dark:text-zinc-400">{formatDateTime(inc.triggered_at)}</div>
                  </div>
                  <IncidentStatusBadge incident={inc} />
                </Link>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card>
        <h2 className="mb-3 text-sm font-medium text-zinc-700 dark:text-zinc-300">Health digests</h2>
        {digests.loading && <Spinner />}
        {digests.data && digests.data.digests.length === 0 && (
          <EmptyState>No health digests yet — the first one generates at 08:00 UTC.</EmptyState>
        )}
        {digests.data && digests.data.digests.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="text-xs text-zinc-500 dark:text-zinc-400">
                <tr>
                  <th className="pb-2 pr-4 font-medium">Period</th>
                  <th className="pb-2 pr-4 font-medium">Status</th>
                  <th className="pb-2 pr-4 font-medium">Checks</th>
                  <th className="pb-2 font-medium">Avg latency</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-100 dark:divide-zinc-800">
                {digests.data.digests.map((d) => (
                  <tr key={d.period_start}>
                    <td className="py-2 pr-4 whitespace-nowrap">{formatDateTime(d.period_start)}</td>
                    <td className="py-2 pr-4">
                      <Badge tone={d.status === "healthy" ? "good" : "warn"}>{d.status}</Badge>
                    </td>
                    <td className="py-2 pr-4">
                      {d.success_count}/{d.total_checks}
                    </td>
                    <td className="py-2">{d.avg_latency_ms != null ? `${d.avg_latency_ms} ms` : "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}

function StatTile({ label, value }: { label: string; value: string }) {
  return (
    <Card>
      <div className="text-xs text-zinc-500 dark:text-zinc-400">{label}</div>
      <div className="mt-1 text-lg font-semibold">{value}</div>
    </Card>
  );
}
