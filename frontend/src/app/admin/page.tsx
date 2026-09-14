"use client";

import { useState } from "react";
import { RequireAuth, useAuth } from "@/lib/auth";
import { ApiError, adminListUsers, adminStats, adminUpdateUser, type AdminUserView } from "@/lib/api";
import { useAsync } from "@/lib/useAsync";
import { formatDateTime } from "@/lib/format";
import { Badge, Button, Card, ErrorBanner, Spinner } from "@/components/ui";

export default function AdminPage() {
  return (
    <RequireAuth>
      <AdminGate />
    </RequireAuth>
  );
}

/** RequireAuth only confirms a session exists; this confirms it's an admin
 * one. The API enforces this regardless — this just avoids a pointless
 * round trip and shows a clearer message than a raw 403 would. */
function AdminGate() {
  const { user } = useAuth();
  if (user?.role !== "admin") {
    return (
      <div className="mx-auto max-w-md py-24 text-center text-sm text-zinc-500 dark:text-zinc-400">
        You need admin access to view this page.
      </div>
    );
  }
  return <AdminDashboard selfId={user.id} />;
}

function AdminDashboard({ selfId }: { selfId: string }) {
  const stats = useAsync(adminStats, []);
  const users = useAsync(adminListUsers, []);
  const [actionError, setActionError] = useState<string | null>(null);

  async function toggleActive(u: AdminUserView) {
    setActionError(null);
    try {
      await adminUpdateUser(u.id, { is_active: !u.is_active });
      users.reload();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Something went wrong.");
    }
  }

  async function toggleRole(u: AdminUserView) {
    setActionError(null);
    try {
      await adminUpdateUser(u.id, { role: u.role === "admin" ? "user" : "admin" });
      users.reload();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : "Something went wrong.");
    }
  }

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold tracking-tight">Admin</h1>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <StatTile label="Total users" value={stats.data ? String(stats.data.total_users) : "—"} />
        <StatTile label="Active endpoints" value={stats.data ? String(stats.data.active_endpoints) : "—"} />
        <StatTile label="Incidents (24h)" value={stats.data ? String(stats.data.incidents_last_24h) : "—"} />
      </div>

      <ErrorBanner message={actionError ?? users.error} />
      {users.loading && <Spinner />}

      {users.data && (
        <Card className="!p-0">
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="text-xs text-zinc-500 dark:text-zinc-400">
                <tr className="border-b border-zinc-200 dark:border-zinc-800">
                  <th className="px-5 py-3 font-medium">Email</th>
                  <th className="px-5 py-3 font-medium">Role</th>
                  <th className="px-5 py-3 font-medium">Status</th>
                  <th className="px-5 py-3 font-medium">Joined</th>
                  <th className="px-5 py-3 font-medium">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-100 dark:divide-zinc-800">
                {users.data.users.map((u) => {
                  const isSelf = u.id === selfId;
                  return (
                    <tr key={u.id}>
                      <td className="px-5 py-3">
                        {u.email}
                        {isSelf && <span className="ml-1.5 text-xs text-zinc-400 dark:text-zinc-600">(you)</span>}
                      </td>
                      <td className="px-5 py-3">
                        <Badge tone={u.role === "admin" ? "good" : "neutral"}>{u.role}</Badge>
                      </td>
                      <td className="px-5 py-3">
                        <Badge tone={u.is_active ? "good" : "bad"}>{u.is_active ? "active" : "disabled"}</Badge>
                      </td>
                      <td className="px-5 py-3 whitespace-nowrap text-zinc-500 dark:text-zinc-400">
                        {formatDateTime(u.created_at)}
                      </td>
                      <td className="px-5 py-3">
                        <div className="flex gap-2">
                          <Button
                            variant="secondary"
                            className="px-2 py-1 text-xs"
                            disabled={isSelf && u.is_active}
                            title={isSelf && u.is_active ? "You can't disable your own account" : undefined}
                            onClick={() => toggleActive(u)}
                          >
                            {u.is_active ? "Disable" : "Enable"}
                          </Button>
                          <Button
                            variant="secondary"
                            className="px-2 py-1 text-xs"
                            disabled={isSelf && u.role === "admin"}
                            title={isSelf && u.role === "admin" ? "You can't remove your own admin access" : undefined}
                            onClick={() => toggleRole(u)}
                          >
                            {u.role === "admin" ? "Remove admin" : "Make admin"}
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </Card>
      )}
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
