import { type RefObject, useEffect, useRef, useState } from "react";
import { formatDuration } from "../../lib/format";
import { Panel } from "../../ui/Panel";
import { KillIcons, killLabel, playKill } from "../Player";
import { type AnalysisStats, activeKill, type ClipAnalysis, isMine, type Kill } from "./analysis";
import { iconUrl, modifierName, weaponName } from "./names";

/** The current time of a video, updated as it plays and seeks. */
function useVideoTime(videoRef: RefObject<HTMLVideoElement | null>) {
  const [time, setTime] = useState(0);
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const update = () => setTime(v.currentTime);
    v.addEventListener("timeupdate", update);
    v.addEventListener("seeked", update);
    return () => {
      v.removeEventListener("timeupdate", update);
      v.removeEventListener("seeked", update);
    };
  }, [videoRef]);
  return time;
}

/** The uploader's stats and every kill from the clip's killfeed, beside the player. */
export function KillfeedPanel({
  analysis,
  videoRef,
}: {
  analysis: ClipAnalysis;
  videoRef: RefObject<HTMLVideoElement | null>;
}) {
  if (analysis.status === "pending" || !analysis.stats) {
    return (
      <section className="glass squircle flex items-center gap-3 rounded-[20px] px-4 py-3 text-sm">
        <span className="h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-muted border-t-accent" />
        <span>
          Reading the killfeed…{" "}
          <span className="text-muted">Stats and kill marks show up here when it's done.</span>
        </span>
      </section>
    );
  }
  return (
    <Panel padding="p-[18px]" className="flex h-full min-h-0 flex-col gap-3.5">
      <Stats stats={analysis.stats} />
      {analysis.kills.length > 0 && <KillList kills={analysis.kills} videoRef={videoRef} />}
    </Panel>
  );
}

function Stat({ value, label }: { value: number; label: string }) {
  return (
    <div className="flex flex-col">
      <span className="text-3xl leading-none font-extrabold tabular-nums">{value}</span>
      <span className="text-xs text-muted">{label}</span>
    </div>
  );
}

function Stats({ stats }: { stats: AnalysisStats }) {
  const headshots = stats.modifiers.headshot ?? 0;
  const none = stats.myKills === 0 && stats.myDeaths === 0;
  return (
    <section className="squircle flex flex-col gap-3 rounded-[14px] bg-white/5 px-4 py-3.5">
      <div className="flex flex-col gap-0.5">
        <div className="flex items-center gap-2">
          <h2 className="text-[17px] font-bold">Your stats</h2>
          {stats.multiKill && (
            <span className="rounded-full bg-accent px-2.5 py-px text-xs font-extrabold text-[#0e0f11] uppercase">
              {stats.multiKill}
            </span>
          )}
        </div>
        <span className="font-mono text-[10px] text-muted">
          from the killfeed · {stats.kills} {stats.kills === 1 ? "kill" : "kills"} in the clip
        </span>
      </div>
      {none ? (
        <p className="text-sm text-muted">
          No kills by you in this clip
          {stats.kills > 0 ? "; the others are marked on the timeline." : "."}
        </p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <Stat value={stats.myKills} label={stats.myKills === 1 ? "kill" : "kills"} />
            <Stat value={stats.myDeaths} label={stats.myDeaths === 1 ? "death" : "deaths"} />
            <Stat value={headshots} label={headshots === 1 ? "headshot" : "headshots"} />
          </div>
          {(Object.keys(stats.weapons).length > 0 || Object.keys(stats.modifiers).length > 0) && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-[13px] text-soft">
              {Object.entries(stats.weapons).map(([w, n]) => (
                <Counted key={w} name={w} label={weaponName(w)} count={n} />
              ))}
              {Object.entries(stats.modifiers).map(([m, n]) => (
                <Counted key={m} name={m} label={modifierName(m)} count={n} muted />
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}

function Counted({
  name,
  label,
  count,
  muted,
}: {
  name: string;
  label: string;
  count: number;
  muted?: boolean;
}) {
  const url = iconUrl(name);
  return (
    <span className="flex items-center gap-1.5" title={`${label} × ${count}`}>
      {url ? <img src={url} alt={label} className="h-[18px] w-auto" /> : label}
      <span className={muted ? "text-muted tabular-nums" : "font-semibold tabular-nums"}>
        ×{count}
      </span>
    </span>
  );
}

function KillList({
  kills,
  videoRef,
}: {
  kills: Kill[];
  videoRef: RefObject<HTMLVideoElement | null>;
}) {
  const time = useVideoTime(videoRef);
  const mineCount = kills.filter(isMine).length;
  // Your kills first: they're what the clip is about.
  const [onlyMine, setOnlyMine] = useState(mineCount > 0 && mineCount < kills.length);
  // Keyed by place in the analysis: two kills can share a second, an owner and a gun.
  const shown = kills.map((k, i) => ({ k, i })).filter(({ k }) => !onlyMine || isMine(k));
  const active = activeKill(kills, time);
  const list = useRef<HTMLDivElement>(null);

  // Keep the kill being watched in view when the list scrolls in its own box (beside the
  // player). Only the list scrolls: never the page, which on phones holds the list below.
  useEffect(() => {
    const box = list.current;
    if (!active || !box || box.scrollHeight <= box.clientHeight) return;
    const row = box.querySelector("[aria-current=true]");
    if (!row) return;
    const r = row.getBoundingClientRect();
    const b = box.getBoundingClientRect();
    if (r.bottom > b.bottom) box.scrollTop += r.bottom - b.bottom;
    else if (r.top < b.top) box.scrollTop -= b.top - r.top;
  }, [active]);

  const seg = (on: boolean) =>
    `h-7 rounded-full px-3 text-[13px] font-semibold ${on ? "bg-white/14 text-text" : "text-soft hover:text-text"}`;
  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between pb-1.5">
        <h2 className="text-[17px] font-bold">Kills</h2>
        {mineCount > 0 && mineCount < kills.length && (
          <div className="flex gap-0.5 rounded-full bg-white/6 p-[3px]">
            <button
              type="button"
              className={seg(onlyMine)}
              aria-pressed={onlyMine}
              onClick={() => setOnlyMine(true)}
            >
              Yours {mineCount}
            </button>
            <button
              type="button"
              className={seg(!onlyMine)}
              aria-pressed={!onlyMine}
              onClick={() => setOnlyMine(false)}
            >
              All {kills.length}
            </button>
          </div>
        )}
      </div>
      <div ref={list} className="-mx-2 flex min-h-0 flex-col gap-0.5 overflow-y-auto">
        {shown.map(({ k, i }) => (
          <KillRow
            key={i}
            kill={k}
            active={k === active}
            // In "Yours" every row is yours; say so only in the full list.
            label={!onlyMine}
            onClick={() => playKill(videoRef.current, k)}
          />
        ))}
      </div>
    </section>
  );
}

/** One kill, drawn the way CS2's killfeed draws it: red outline for your kills, a dark
 *  red fill for your deaths. Names aren't read, so "You" stands for the uploader. */
function KillRow({
  kill,
  active,
  label,
  onClick,
}: {
  kill: Kill;
  active: boolean;
  label: boolean;
  onClick: () => void;
}) {
  const you = <span className="text-[13px] font-bold text-text">You</span>;
  const chip =
    kill.owner === "myKill"
      ? "bg-[rgba(10,10,12,0.8)] shadow-[inset_0_0_0_1.5px_#e5484d]"
      : kill.owner === "myDeath"
        ? "bg-[#78080c]/90"
        : "bg-[rgba(10,10,12,0.8)]";
  return (
    <button
      type="button"
      aria-current={active}
      onClick={onClick}
      title={`Jump to ${formatDuration(kill.t * 1000)}: ${killLabel(kill)}`}
      className={`squircle flex min-h-10 w-full items-center gap-3 rounded-xl px-2.5 py-1.5 text-left ${
        active ? "bg-accent/10 ring-1 ring-accent/35 ring-inset" : "hover:bg-white/6"
      }`}
    >
      <span
        className={`w-[34px] font-mono text-[11px] tabular-nums ${active ? "text-text" : "text-muted"}`}
      >
        {formatDuration(kill.t * 1000)}
      </span>
      <span className={`flex h-7 items-center gap-[7px] rounded px-2.5 ${chip}`}>
        {kill.owner === "myKill" && you}
        <KillIcons kill={kill} size={16} />
        {kill.owner === "myDeath" && you}
      </span>
      {(kill.owner === "myDeath" || (label && kill.owner === "myKill")) && (
        <span
          className={`ml-auto text-xs font-semibold ${kill.owner === "myKill" ? "text-accent" : "text-danger"}`}
        >
          {kill.owner === "myKill" ? "Your kill" : "Your death"}
        </span>
      )}
    </button>
  );
}
