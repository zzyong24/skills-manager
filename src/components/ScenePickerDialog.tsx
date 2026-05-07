import { useState, useEffect } from "react";
import { X, Layers, Search, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "../utils";
import * as api from "../lib/tauri";

interface Props {
  boundSceneIds: string[];
  onBind: (scenarioId: string) => Promise<void>;
  onClose: () => void;
}

export function ScenePickerDialog({ boundSceneIds, onBind, onClose }: Props) {
  const { t } = useTranslation();
  const [scenes, setScenes] = useState<{ id: string; name: string; skill_count: number }[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [binding, setBinding] = useState<string | null>(null);

  useEffect(() => {
    const load = async () => {
      try {
        const allScenes = await api.getScenarios();
        setScenes(allScenes);
      } catch (e) {
        console.error("Failed to load scenes:", e);
      } finally {
        setLoading(false);
      }
    };
    load();
  }, []);

  const availableScenes = scenes.filter(
    (s) => !boundSceneIds.includes(s.id) && s.name.toLowerCase().includes(search.toLowerCase())
  );

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-surface border border-border rounded-xl w-full max-w-sm p-5 shadow-2xl">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-[13px] font-semibold text-primary flex items-center gap-2">
            <Layers className="w-4 h-4 text-muted" />
            {t("project.bindScene")}
          </h2>
          <button
            onClick={onClose}
            className="text-muted hover:text-secondary p-1 rounded transition-colors outline-none"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="mb-4">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted" />
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("project.searchScenes")}
              className="app-input w-full pl-9"
              autoCapitalize="none"
            />
          </div>
        </div>

        <div className="max-h-[300px] space-y-1 overflow-y-auto">
          {loading ? (
            <p className="text-center text-[13px] text-muted">{t("common.loading")}</p>
          ) : availableScenes.length === 0 ? (
            <p className="text-center text-[13px] text-muted">{t("project.noAvailableScenes")}</p>
          ) : (
            availableScenes.map((scene) => (
              <button
                key={scene.id}
                onClick={async () => {
                  setBinding(scene.id);
                  try {
                    await onBind(scene.id);
                  } finally {
                    setBinding(null);
                  }
                }}
                disabled={binding !== null}
                className={cn(
                  "flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-[13px] transition-colors",
                  binding === scene.id
                    ? "bg-surface-hover text-muted"
                    : "hover:bg-surface-hover text-secondary"
                )}
              >
                <Layers className="h-4 w-4 text-muted" />
                <span className="flex-1 font-medium">{scene.name}</span>
                <span className="text-[12px] text-muted">{scene.skill_count} skills</span>
                {binding === scene.id && (
                  <Loader2 className="h-3 w-3 animate-spin text-muted" />
                )}
              </button>
            ))
          )}
        </div>

        <div className="mt-4 flex justify-end">
          <button
            onClick={onClose}
            className="px-3 py-1.5 rounded-[4px] text-[13px] font-medium text-tertiary hover:text-secondary hover:bg-surface-hover transition-colors outline-none"
          >
            {t("common.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
