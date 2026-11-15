// Settings › Brands (docs/adr/brands.md): the brands agents build decks and documents with, the
// default one, a new brand (a brand chat in the workspace shown, where the user gives what they
// have), and removing one. Decks made with a removed brand keep their copy.

import type { BrandsResult } from "@chauffeur/protocol";
import { Palette, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { client, openSettings, selectChat, selectWorkspace, toast, useApp } from "@/state/app";
import { Button, Field, IconButton, inputClass, Select } from "./ui";

const NONE = "__none__";

export function BrandsSettings() {
  const [brands, setBrands] = useState<BrandsResult | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const workspace = useApp((s) => (s.selectedId ? s.workspaces[s.selectedId] : undefined));
  useEffect(() => {
    void client.call("brands.list", {}).then(setBrands).catch((error: unknown) => toast(`Could not read the brands: ${errorMessage(error)}`, "error"));
  }, []);
  const act = async (what: string, run: () => Promise<void>) => {
    setBusy(true);
    try {
      await run();
    } catch (error) {
      toast(`Could not ${what}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  const create = () =>
    act("create the brand", async () => {
      if (!workspace) return;
      const chat = await client.call("brands.create", { name, workspace: workspace.id });
      setName("");
      openSettings(false);
      selectWorkspace(workspace.id);
      selectChat(workspace.id, chat.id);
      toast("Give the brand chat what you have: a deck, guidelines, a website, a logo");
    });
  return (
    <div className="flex flex-col gap-4" data-testid="brands-settings">
      <div>
        <div className="mb-1 font-medium text-fg">Brands</div>
        <p className="text-[12px] leading-relaxed text-muted">
          A brand gives the decks and documents agents make one look: colours, fonts, logos, tone and slide models. Each deck copies its brand, so changing a brand never changes a deck already made.
        </p>
      </div>
      {brands && brands.brands.length > 0 && (
        <Field label="Default brand" hint="Used by every chat that did not choose another one in its composer.">
          <Select
            value={brands.default ?? NONE}
            options={[{ value: NONE, label: "None" }, ...brands.brands.map((b) => ({ value: b.id, label: b.name }))]}
            onChange={(id) => void act("set the default brand", async () => setBrands(await client.call("brands.set_default", { id: id === NONE ? undefined : id })))}
            testId="brands-default"
          />
        </Field>
      )}
      <ul className="flex flex-col gap-1">
        {brands?.brands.map((brand) => (
          <li key={brand.id} className="flex items-center gap-2 rounded-md border border-line px-2.5 py-1.5 text-[12.5px]" data-testid="brand-row">
            <Palette size={13} className="text-muted" />
            <span className="min-w-0 flex-1 truncate">{brand.name}</span>
            {brands.default === brand.id && <span className="text-[11px] text-muted">default</span>}
            <IconButton title={`Delete ${brand.name}`} disabled={busy} onClick={() => {
              if (!window.confirm(`Delete the brand ${brand.name}? Decks made with it keep their copy.`)) return;
              void act("delete the brand", async () => setBrands(await client.call("brands.delete", { id: brand.id })));
            }}><Trash2 size={13} /></IconButton>
          </li>
        ))}
      </ul>
      <Field label="New brand" hint={workspace ? `Its chat opens in ${workspace.title ?? workspace.slug}: give it an existing deck, brand guidelines, a website or a logo.` : "Open a workspace first: the brand's chat opens there."}>
        <div className="flex gap-2">
          <input className={inputClass} value={name} onChange={(event) => setName(event.target.value)} placeholder="Acme" onKeyDown={(event) => { if (event.key === "Enter" && name.trim() && workspace) void create(); }} data-testid="brand-name" />
          <Button variant="primary" disabled={!name.trim() || !workspace} busy={busy} onClick={() => void create()}><Plus size={13} /> Create</Button>
        </div>
      </Field>
    </div>
  );
}

const DEFAULT = "__default__";

/** The composer's choice of the brand a chat builds its decks and documents with. */
export function BrandPicker({ chat }: { chat: { id: string; brand?: string } }) {
  const [brands, setBrands] = useState<BrandsResult | null>(null);
  useEffect(() => {
    void client.call("brands.list", {}).then(setBrands).catch(() => {});
  }, []);
  if (!brands || brands.brands.length === 0) return null;
  const fallback = brands.brands.find((b) => b.id === brands.default);
  const options = [
    { value: DEFAULT, label: fallback ? `Brand: ${fallback.name}` : "No brand", name: fallback ? `Default (${fallback.name})` : "Default (none)" },
    ...brands.brands.map((b) => ({ value: b.id, label: `Brand: ${b.name}`, name: b.name })),
    { value: "none", label: "No brand", name: "None" },
  ];
  return (
    <Select
      value={chat.brand ?? DEFAULT}
      options={options}
      title="The brand this chat's decks and documents are made with, from its next session"
      onChange={(value) => void client.call("chat.set_brand", { chat: chat.id, brand: value === DEFAULT ? undefined : value })
        .catch((error: unknown) => toast(`Could not change the brand: ${errorMessage(error)}`, "error"))}
      testId="brand-picker"
    />
  );
}
