// Settings › Brands (docs/adr/brands.md): the brands agents build decks and documents with, the
// default one, a new brand or editing one (its chat in the brand editor, where the user gives what
// they have), and removing one. Decks made with a removed brand keep their copy.

import type { BrandsResult } from "@chauffeur/protocol";
import { Palette, Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { client, openBrandEditor, toast } from "@/state/app";
import { Button, Field, IconButton, inputClass, Select } from "./ui";

const NONE = "__none__";

export function BrandsSettings() {
  const [brands, setBrands] = useState<BrandsResult | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
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
      const chat = await client.call("brands.create", { name });
      const id = chat.brand ?? "";
      setName("");
      openBrandEditor({ id, name: name.trim() }, chat);
      toast("Give the brand chat what you have: a deck, guidelines, a website, a logo");
    });
  const edit = (brand: { id: string; name: string }) =>
    act("open the brand", async () => openBrandEditor(brand, await client.call("brands.edit", { id: brand.id })));
  return (
    <div className="flex flex-col gap-4" data-testid="brands-settings">
      <div>
        <div className="mb-1 font-medium text-fg">Brands</div>
        <p className="text-[12px] leading-relaxed text-muted">
          A brand gives the decks and documents agents make one look: colours, fonts, logos, tone and slide models. Each deck copies its brand, so changing a brand never changes a deck already made.
        </p>
      </div>
      {brands && brands.brands.length > 0 && (
        <Field label="Default brand" hint="Chats build their decks and documents with it. With several brands, the agent asks which one before making a deck or a document.">
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
            <IconButton title={`Edit ${brand.name}`} disabled={busy} onClick={() => void edit(brand)} data-testid="brand-edit"><Pencil size={13} /></IconButton>
            <IconButton title={`Delete ${brand.name}`} disabled={busy} onClick={() => {
              if (!window.confirm(`Delete the brand ${brand.name}? Decks made with it keep their copy.`)) return;
              void act("delete the brand", async () => setBrands(await client.call("brands.delete", { id: brand.id })));
            }}><Trash2 size={13} /></IconButton>
          </li>
        ))}
      </ul>
      <Field label="New brand" hint="Its chat opens in the brand editor: give it an existing deck, brand guidelines, a website or a logo.">
        <div className="flex gap-2">
          <input className={inputClass} value={name} onChange={(event) => setName(event.target.value)} placeholder="Acme" onKeyDown={(event) => { if (event.key === "Enter" && name.trim()) void create(); }} data-testid="brand-name" />
          <Button variant="primary" disabled={!name.trim()} busy={busy} onClick={() => void create()}><Plus size={13} /> Create</Button>
        </div>
      </Field>
    </div>
  );
}
