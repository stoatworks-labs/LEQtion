import { useState } from 'react';

import { useStore } from '../state/store';

/**
 * Projects and shows — the container the tuning work lives in.
 *
 * A **project** is a folder grouping shows; a **show** is a complete configuration
 * (engine, transfer, generator, input, tiles) saved under a name. See
 * `docs/tuning.md` §1.
 *
 * Three things this bar is careful about:
 *
 * - **Nothing here is required to measure.** With no project open the app meters,
 *   logs and calibrates exactly as it always has, and this bar says so rather than
 *   nagging. Someone opening a meter to check a level should not have to name a
 *   project first.
 * - **Loading a show replaces the current configuration**, so when there are unsaved
 *   changes it asks first — inline, because an unanswered modal over a running
 *   measurement is worse than the question.
 * - **Deleting says where it went.** A project or show is moved to a `.deleted`
 *   folder rather than unlinked, and a button labelled "Delete" that actually means
 *   "move" has to admit it.
 */
export function ProjectBar() {
  const projects = useStore((s) => s.projects);
  const project = useStore((s) => s.project);
  const shows = useStore((s) => s.shows);
  const activeShow = useStore((s) => s.activeShow);
  const showChanged = useStore((s) => s.showChanged);
  const projectsRoot = useStore((s) => s.projectsRoot);

  const openProject = useStore((s) => s.openProject);
  const closeProject = useStore((s) => s.closeProject);
  const createProject = useStore((s) => s.createProject);
  const renameProject = useStore((s) => s.renameProject);
  const deleteProject = useStore((s) => s.deleteProject);
  const loadShow = useStore((s) => s.loadShow);
  const saveShowAs = useStore((s) => s.saveShowAs);
  const updateActiveShow = useStore((s) => s.updateActiveShow);
  const renameShow = useStore((s) => s.renameShow);
  const deleteShow = useStore((s) => s.deleteShow);

  const [panel, setPanel] = useState<'none' | 'new' | 'saveAs' | 'manage'>('none');
  const [name, setName] = useState('');
  /** A show waiting on confirmation because loading it would discard changes. */
  const [pendingLoad, setPendingLoad] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const toggle = (p: typeof panel) => {
    setName('');
    setPanel(panel === p ? 'none' : p);
  };

  function chooseShow(id: string) {
    if (!id) return;
    if (showChanged && activeShow) {
      setPendingLoad(id);
      return;
    }
    void loadShow(id);
  }

  return (
    <div className="projectbar">
      <label>
        Project
        <select
          value={project?.dir ?? ''}
          onChange={(e) => {
            const dir = e.target.value;
            setNote(null);
            void (dir ? openProject(dir) : closeProject());
          }}
        >
          <option value="">— no project —</option>
          {projects.map((p) => (
            <option key={p.dir} value={p.dir}>
              {p.name} ({p.showCount})
            </option>
          ))}
        </select>
      </label>

      <button type="button" onClick={() => toggle('new')} aria-pressed={panel === 'new'}>
        New project…
      </button>

      <label className="grow">
        Show
        <select
          value={activeShow?.id ?? ''}
          onChange={(e) => chooseShow(e.target.value)}
          disabled={!project}
        >
          <option value="">
            {project ? (shows.length ? '— choose a show —' : 'no shows yet') : 'no project open'}
          </option>
          {shows.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
              {s.device ? ` · ${s.device}` : ''}
            </option>
          ))}
        </select>
      </label>

      <button
        type="button"
        onClick={() => void updateActiveShow()}
        disabled={!activeShow || !showChanged}
        title={
          activeShow
            ? `Overwrite "${activeShow.name}" with the current configuration`
            : 'No show is loaded'
        }
      >
        Save
      </button>

      <button
        type="button"
        onClick={() => toggle('saveAs')}
        aria-pressed={panel === 'saveAs'}
        disabled={!project}
        title={project ? undefined : 'Open or create a project first'}
      >
        Save as…
      </button>

      <button
        type="button"
        onClick={() => toggle('manage')}
        aria-pressed={panel === 'manage'}
        disabled={!project}
      >
        Manage…
      </button>

      <span className="spacer" />

      {activeShow && showChanged && (
        <span className="chip warn" title="The configuration differs from the saved show">
          unsaved changes
        </span>
      )}
      {activeShow && !showChanged && <span className="chip good">saved</span>}
      {!project && (
        <span className="chip" title={projectsRoot || undefined}>
          measuring without a project
        </span>
      )}

      {note && (
        <p className="hint bar-note">
          {note}{' '}
          <button type="button" className="icon" aria-label="Dismiss" onClick={() => setNote(null)}>
            ✕
          </button>
        </p>
      )}

      {pendingLoad && (
        <div className="panel confirm" role="alertdialog" aria-label="Discard changes?">
          <p className="hint">
            <strong>{activeShow?.name}</strong> has unsaved changes. Loading another show
            replaces the current configuration.
          </p>
          <button
            type="button"
            onClick={() => {
              void updateActiveShow().then(() => {
                void loadShow(pendingLoad);
                setPendingLoad(null);
              });
            }}
          >
            Save, then load
          </button>
          <button
            type="button"
            className="primary"
            onClick={() => {
              void loadShow(pendingLoad);
              setPendingLoad(null);
            }}
          >
            Discard and load
          </button>
          <button type="button" onClick={() => setPendingLoad(null)}>
            Cancel
          </button>
        </div>
      )}

      {panel === 'new' && (
        <NamePanel
          label="Project name"
          action="Create"
          value={name}
          onChange={setName}
          onSubmit={() => {
            void createProject(name);
            setPanel('none');
            setName('');
          }}
          hint={projectsRoot ? `Created in ${projectsRoot}` : undefined}
        />
      )}

      {panel === 'saveAs' && (
        <NamePanel
          label="Show name"
          action="Save"
          value={name}
          onChange={setName}
          onSubmit={() => {
            void saveShowAs(name);
            setPanel('none');
            setName('');
          }}
          hint="Saves the engine, transfer, generator, input and tile layout. The generator's signal is not saved as running — a show never starts one when it loads."
        />
      )}

      {panel === 'manage' && project && (
        <div className="panel manage">
          <label className="grow">
            Rename project
            <input
              type="text"
              defaultValue={project.name}
              onBlur={(e) => {
                const next = e.target.value.trim();
                if (next && next !== project.name) void renameProject(next);
              }}
            />
          </label>
          <button
            type="button"
            onClick={() => {
              void deleteProject(project.dir).then((movedTo) => {
                if (movedTo) setNote(`Project moved to ${movedTo}. Nothing was erased.`);
                setPanel('none');
              });
            }}
          >
            Delete project
          </button>

          <table className="leq-table">
            <thead>
              <tr>
                <th>Show</th>
                <th>Saved</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {shows.map((s) => (
                <tr key={s.id}>
                  <td>
                    <input
                      type="text"
                      defaultValue={s.name}
                      onBlur={(e) => {
                        const next = e.target.value.trim();
                        if (next && next !== s.name) void renameShow(s.id, next);
                      }}
                    />
                  </td>
                  <td className="hint">{s.modified.slice(0, 16).replace('T', ' ')}</td>
                  <td>
                    <button
                      type="button"
                      className="icon"
                      aria-label={`Delete ${s.name}`}
                      onClick={() => {
                        void deleteShow(s.id).then((movedTo) => {
                          if (movedTo) setNote(`"${s.name}" moved to ${movedTo}. Nothing was erased.`);
                        });
                      }}
                    >
                      ✕
                    </button>
                  </td>
                </tr>
              ))}
              {shows.length === 0 && (
                <tr>
                  <td colSpan={3} className="hint">
                    No shows in this project yet. Set the app up how you want it and use
                    “Save as…”.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function NamePanel({
  label,
  action,
  value,
  onChange,
  onSubmit,
  hint,
}: {
  label: string;
  action: string;
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  hint?: string;
}) {
  return (
    <div className="panel">
      <label className="grow">
        {label}
        <input
          type="text"
          value={value}
          autoFocus
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && value.trim()) onSubmit();
          }}
        />
      </label>
      <button type="button" className="primary" disabled={!value.trim()} onClick={onSubmit}>
        {action}
      </button>
      {hint && <p className="hint">{hint}</p>}
    </div>
  );
}
