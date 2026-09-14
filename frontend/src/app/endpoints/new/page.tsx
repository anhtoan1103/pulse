"use client";

import { useRouter } from "next/navigation";
import { RequireAuth } from "@/lib/auth";
import { createEndpoint, type EndpointInput } from "@/lib/api";
import { EndpointForm } from "@/components/EndpointForm";
import { Card } from "@/components/ui";

export default function NewEndpointPage() {
  const router = useRouter();

  return (
    <RequireAuth>
      <div className="mx-auto max-w-2xl">
        <h1 className="mb-6 text-xl font-semibold tracking-tight">Add endpoint</h1>
        <Card>
          <EndpointForm
            submitLabel="Create endpoint"
            onSubmit={async (input) => {
              // No `initial` passed to this form, so it always submits every
              // field (see EndpointForm's onSubmit doc comment).
              const created = await createEndpoint(input as EndpointInput);
              router.push(`/endpoints/${created.id}`);
            }}
          />
        </Card>
      </div>
    </RequireAuth>
  );
}
