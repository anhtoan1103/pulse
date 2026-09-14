"use client";

import { useState } from "react";
import Link from "next/link";
import { RequireAuth } from "@/lib/auth";
import { listIncidents } from "@/lib/api";
import { useAsync } from "@/lib/useAsync";
import { formatDateTime, triggerReasonLabel } from "@/lib/format";
import { IncidentStatusBadge } from "@/components/IncidentStatusBadge";
import { Badge, Card, EmptyState, ErrorBanner, Select, Spinner } from "@/components/ui";

type StatusFilter = "all" | "open" | "resolved";

export default function IncidentsPage() {
  return (
    <RequireAuth>
      <IncidentsList />
    </RequireAuth>
  );
}

function IncidentsList() {
  const [status, setStatus] = useState<StatusFilter>("all");
  const { data, error, loading } = useAsync(
    () => listIncidents(status === "all" ? {} : { status }),
    [status],
  );

  return (
    <div>
      <div className="mb-6 flex items-center justify-between gap-4">
        <h1 className="text-xl font-semibold tracking-tight">Incidents</h1>
        {/* Fixed-width wrapper, not a Select className override: the
            component's own `w-full` otherwise wins the cascade regardless
            of class order in the attribute (Tailwind's stylesheet order,
            not markup order, decides ties). */}
        <div className="w-36 shrink-0">
          <Select value={status} onChange={(e) => setStatus(e.target.value as StatusFilter)}>
            <option value="all">All</option>
            <option value="open">Open</option>
            <option value="resolved">Resolved</option>
          </Select>
        </div>
      </div>

      <ErrorBanner message={error} />
      {loading && <Spinner />}

      {data && data.incidents.length === 0 && <EmptyState>No incidents to show.</EmptyState>}

      {data && data.incidents.length > 0 && (
        <Card className="divide-y divide-zinc-200 !p-0 dark:divide-zinc-800">
          {data.incidents.map((inc) => (
            <Link
              key={inc.id}
              href={`/incidents/${inc.id}`}
              className="flex items-center justify-between gap-4 px-5 py-4 hover:bg-zinc-50 dark:hover:bg-zinc-800/50"
            >
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{triggerReasonLabel(inc.trigger_reason)}</span>
                  <IncidentStatusBadge incident={inc} />
                </div>
                <div className="truncate text-sm text-zinc-500 dark:text-zinc-400">{inc.endpoint_name}</div>
              </div>
              <div className="shrink-0 text-right text-xs text-zinc-500 dark:text-zinc-400">
                <div>{formatDateTime(inc.triggered_at)}</div>
                <AiStatusBadge status={inc.ai_status} />
              </div>
            </Link>
          ))}
        </Card>
      )}
    </div>
  );
}

function AiStatusBadge({ status }: { status: string }) {
  if (status === "completed") return <Badge tone="good">AI analyzed</Badge>;
  if (status === "failed") return <Badge tone="warn">AI failed</Badge>;
  return <Badge>AI pending</Badge>;
}
