"use client";

import { use, useState } from "react";
import Link from "next/link";
import { RequireAuth } from "@/lib/auth";
import { ApiError, getIncident, resolveIncident, type MetricSnapshot } from "@/lib/api";
import { useAsync } from "@/lib/useAsync";
import { formatDateTime, triggerReasonLabel } from "@/lib/format";
import { IncidentStatusBadge } from "@/components/IncidentStatusBadge";
import { Badge, Button, Card, ErrorBanner, Spinner } from "@/components/ui";

export default function IncidentDetailPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  return (
    <RequireAuth>
      <IncidentDetail id={id} />
    </RequireAuth>
  );
}

function IncidentDetail({ id }: { id: string }) {
  const incident = useAsync(() => getIncident(id), [id]);
  const [actionError, setActionError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);

  if (incident.loading) return <Spinner />;
  if (incident.error || !incident.data) {
    return <ErrorBanner message={incident.error ?? "Incident not found."} />;
  }
  const inc = incident.data;

  async function handleResolve() {
    setActionError(null);
    setResolving(true);
    try {
      await resolveIncident(id);
      incident.reload();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Something went wrong.");
    } finally {
      setResolving(false);
    }
  }

  return (
    <div className="max-w-3xl space-y-6">
      <div>
        <Link
          href={`/endpoints/${inc.endpoint_id}`}
          className="text-sm text-zinc-500 hover:underline dark:text-zinc-400"
        >
          ← {inc.endpoint_name}
        </Link>
        <div className="mt-2 flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-2">
            <h1 className="text-xl font-semibold tracking-tight">{triggerReasonLabel(inc.trigger_reason)}</h1>
            <IncidentStatusBadge incident={inc} />
          </div>
          {!inc.resolved_at && (
            <Button onClick={handleResolve} disabled={resolving}>
              {resolving ? "Resolving…" : "Mark resolved"}
            </Button>
          )}
        </div>
        <p className="mt-1 text-sm text-zinc-500 dark:text-zinc-400">Triggered {formatDateTime(inc.triggered_at)}</p>
      </div>

      <ErrorBanner message={actionError} />

      {inc.recovered_at && !inc.resolved_at && (
        <div className="rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-sm text-emerald-800 dark:border-emerald-900 dark:bg-emerald-950/50 dark:text-emerald-300">
          Metrics returned to normal {formatDateTime(inc.recovered_at)}. Resolve this incident if you&apos;re done
          investigating.
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <MetricCard title="Before" snapshot={inc.metric_before} />
        <MetricCard title="After (triggered detection)" snapshot={inc.metric_after} />
      </div>

      <Card>
        <h2 className="mb-3 text-sm font-medium text-zinc-700 dark:text-zinc-300">AI analysis</h2>
        <AiAnalysis incident={inc} />
      </Card>
    </div>
  );
}

function MetricCard({ title, snapshot }: { title: string; snapshot: MetricSnapshot | null }) {
  return (
    <Card>
      <h3 className="mb-2 text-sm font-medium text-zinc-700 dark:text-zinc-300">{title}</h3>
      {!snapshot ? (
        <p className="text-sm text-zinc-400 dark:text-zinc-600">No data</p>
      ) : (
        <dl className="space-y-1 text-sm">
          <Row label="Period" value={snapshot.period} />
          <Row label="Checks" value={String(snapshot.total_checks)} />
          <Row label="Error rate" value={`${snapshot.error_rate_percent}%`} />
          <Row
            label="Avg latency"
            value={snapshot.avg_latency_ms != null ? `${snapshot.avg_latency_ms} ms` : "n/a"}
          />
        </dl>
      )}
    </Card>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between gap-4">
      <dt className="text-zinc-500 dark:text-zinc-400">{label}</dt>
      <dd className="font-medium">{value}</dd>
    </div>
  );
}

function AiAnalysis({
  incident,
}: {
  incident: {
    ai_status: string;
    ai_possible_cause: string | null;
    ai_confidence: string | null;
    ai_evidence: string[] | null;
    ai_suggested_steps: string[] | null;
    ai_error: string | null;
  };
}) {
  if (incident.ai_status === "pending") {
    return <p className="text-sm text-zinc-500 dark:text-zinc-400">Analysis is still in progress…</p>;
  }
  if (incident.ai_status === "failed") {
    return (
      <p className="text-sm text-zinc-500 dark:text-zinc-400">
        AI analysis was not completed{incident.ai_error ? `: ${incident.ai_error}` : "."} The raw metrics above are
        still accurate.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <p className="text-sm">{incident.ai_possible_cause}</p>
        {incident.ai_confidence && <ConfidenceBadge confidence={incident.ai_confidence} />}
      </div>

      {incident.ai_evidence && incident.ai_evidence.length > 0 && (
        <div>
          <h4 className="mb-1 text-xs font-medium tracking-wide text-zinc-500 uppercase dark:text-zinc-400">
            Evidence
          </h4>
          <ul className="list-disc space-y-0.5 pl-5 text-sm text-zinc-700 dark:text-zinc-300">
            {incident.ai_evidence.map((e, i) => (
              <li key={i}>{e}</li>
            ))}
          </ul>
        </div>
      )}

      {incident.ai_suggested_steps && incident.ai_suggested_steps.length > 0 && (
        <div>
          <h4 className="mb-1 text-xs font-medium tracking-wide text-zinc-500 uppercase dark:text-zinc-400">
            Suggested steps
          </h4>
          <ul className="list-disc space-y-0.5 pl-5 text-sm text-zinc-700 dark:text-zinc-300">
            {incident.ai_suggested_steps.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function ConfidenceBadge({ confidence }: { confidence: string }) {
  const tone = confidence === "high" ? "good" : confidence === "medium" ? "warn" : "neutral";
  return <Badge tone={tone}>{confidence} confidence</Badge>;
}
