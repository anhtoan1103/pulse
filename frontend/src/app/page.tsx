"use client";

import Link from "next/link";
import { RequireAuth } from "@/lib/auth";
import { listEndpoints } from "@/lib/api";
import { useAsync } from "@/lib/useAsync";
import { relativeTime } from "@/lib/format";
import { Badge, Button, Card, EmptyState, ErrorBanner, Spinner } from "@/components/ui";

export default function DashboardPage() {
  return (
    <RequireAuth>
      <EndpointsList />
    </RequireAuth>
  );
}

function EndpointsList() {
  const { data, error, loading } = useAsync(listEndpoints, []);

  return (
    <div>
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-xl font-semibold tracking-tight">Endpoints</h1>
        <Link href="/endpoints/new">
          <Button>Add endpoint</Button>
        </Link>
      </div>

      <ErrorBanner message={error} />
      {loading && <Spinner />}

      {data && data.endpoints.length === 0 && (
        <EmptyState>
          No endpoints yet.{" "}
          <Link href="/endpoints/new" className="font-medium text-zinc-900 underline dark:text-zinc-100">
            Add your first one
          </Link>
          .
        </EmptyState>
      )}

      {data && data.endpoints.length > 0 && (
        <Card className="divide-y divide-zinc-200 !p-0 dark:divide-zinc-800">
          {data.endpoints.map((ep) => (
            <Link
              key={ep.id}
              href={`/endpoints/${ep.id}`}
              className="flex items-center justify-between gap-4 px-5 py-4 hover:bg-zinc-50 dark:hover:bg-zinc-800/50"
            >
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span className="truncate font-medium">{ep.name}</span>
                  {!ep.is_active && <Badge>paused</Badge>}
                </div>
                <div className="truncate text-sm text-zinc-500 dark:text-zinc-400">
                  {ep.method} {ep.url}
                </div>
              </div>
              <div className="shrink-0 text-right text-xs text-zinc-500 dark:text-zinc-400">
                <div>every {ep.check_interval_seconds}s</div>
                <div>last checked {relativeTime(ep.last_checked_at)}</div>
              </div>
            </Link>
          ))}
        </Card>
      )}
    </div>
  );
}
