import React, { useEffect, useRef, useState } from 'react';
import { EQ_HZ, EQ_LABELS, EQ_LIMIT, clampDb, type EqPreset, type PresetBind } from './eq.ts';
import { FX_DEFAULT, SCENES, fxActive, sceneFx, suggestForGenre, type AutoEq, type Fx, type Scene } from './fx.ts';
import type { Song, TagEdit } from '../types';
import {
  addMusicFolder, browseFolder, convertAudio, ffmpegAvailable, listMusicFolders, readTags,
  removeMusicFolder, scanFolders, writeTags, playerPlayUrl, playerSetCrossfade,
  eqResponse,
  listRadio, saveRadio, renameRadio, deleteRadio,
} from '../lib/api';
import type { RadioStation } from '../types';

type PanelTab = 'eq' | 'fx' | 'suggest';

function FxSlider({ label, value, min, max, step, fmt, onChange, disabled, title }: {
  label: string; value: number; min: number; max: number; step: number;
  fmt: (v: number) => string; onChange: (v: number) => void; disabled?: boolean; title?: string;
}) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <label className={`flex items-center gap-2 text-[12px] ${disabled ? 'opacity-40' : ''}`} title={title}>
      <span className="w-14 shrink-0 text-[#8a847a]">{label}</span>
      <input
        type="range" min={min} max={max} step={step} value={value} disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="line min-w-0 w-0 flex-1" style={{ ['--pct' as string]: `${pct}%` }}
      />
      <span className="num w-14 shrink-0 text-right">{fmt(value)}</span>
    </label>
  );
}

function FxCard({ title, on, onToggle, note, children }: {
  title: string; on?: boolean; onToggle?: (v: boolean) => void; note?: string; children?: React.ReactNode;
}) {
  return (
    <div className={`border rounded-sm px-3 py-2 ${on ? 'border-[#e23b2f]/60' : 'border-[#2a2723]'}`}>
      <div className="flex items-center gap-2 mb-1">
        {onToggle && <input type="checkbox" checked={!!on} onChange={(e) => onToggle(e.target.checked)} className="accent-[#e23b2f]" aria-label={title} />}
        <span className={`text-[13px] font-bold ${on ? 'text-[#f4f0e8]' : 'text-[#c9c3b8]'}`}>{title}</span>
        {note && <span className="ml-auto text-[10px] text-[#8a847a] text-right">{note}</span>}
      </div>
      <div className="space-y-1">{children}</div>
    </div>
  );
}

const pctFmt = (v: number) => `${Math.round(v * 100)}%`;
const xFmt = (v: number) => `${v.toFixed(2)}×`;

export function EqPanel({
  bands, preamp, enabled, normalize, crossfade, gain, fx, isRadio,
  presets, selectedPreset, presetDraft, song, auto, autoNote,
  onBands, onPreamp, onEnabled, onNormalize, onCrossfade, onFx, onScene, onAuto,
  onApplyPreset, onSavePreset, onDeletePreset, onPresetNameChange, onClose,
}: {
  bands: number[];
  preamp: number;
  enabled: boolean;
  normalize: boolean;
  crossfade: number;
  gain: number;
  fx: Fx;
  isRadio: boolean;
  presets: EqPreset[];
  selectedPreset: string | null;
  presetDraft: string;
  song: { path?: string | null; album?: string | null; genre?: string | null } | null;
  auto: AutoEq;
  autoNote: string;
  onBands: (b: number[]) => void;
  onPreamp: (v: number) => void;
  onEnabled: (v: boolean) => void;
  onNormalize: (v: boolean) => void;
  onCrossfade: (ms: number) => void;
  onFx: (fx: Fx) => void;
  onScene: (scene: Scene) => void;
  onAuto: (auto: AutoEq) => void;
  onApplyPreset: (name: string) => void;
  onSavePreset: (bind?: PresetBind) => void;
  onDeletePreset: (name: string) => void;
  onPresetNameChange: (v: string) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<PanelTab>('eq');
  const [curve, setCurve] = useState<Array<[number, number]>>([]);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const [bindKind, setBindKind] = useState<'' | PresetBind['kind']>('');
  const drag = useRef<{ dx: number; dy: number } | null>(null);
  const boxRef = useRef<HTMLDivElement | null>(null);

  // 曲線由後端用音訊執行緒同一套濾波器設計算出，畫的就是聽到的。
  useEffect(() => {
    if (tab !== 'eq') return;
    let alive = true;
    const id = window.requestAnimationFrame(() => {
      eqResponse(bands, preamp, enabled).then((pts) => { if (alive) setCurve(pts); }).catch(() => { if (alive) setCurve([]); });
    });
    return () => { alive = false; window.cancelAnimationFrame(id); };
  }, [bands, preamp, enabled, tab]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const set = (i: number, v: number) => {
    const next = bands.slice();
    next[i] = clampDb(v);
    onBands(next);
  };
  const upd = (patch: Partial<Fx>) => onFx({ ...fx, ...patch });
  const fmt = (v: number) => `${v > 0 ? '+' : ''}${Number.isInteger(v) ? v.toFixed(0) : v.toFixed(1)}`;

  const W = 1000;
  const H = 120;
  const RANGE = 20;
  const xOf = (f: number) => (Math.log(f / 20) / Math.log(20000 / 20)) * W;
  const yOf = (db: number) => H / 2 - (Math.max(-RANGE, Math.min(RANGE, db)) / RANGE) * (H / 2 - 4);
  const path = curve.map(([f, db], i) => `${i ? 'L' : 'M'}${xOf(f).toFixed(1)},${yOf(db).toFixed(1)}`).join(' ');
  const peak = curve.reduce((m, [, db]) => Math.max(m, db), -Infinity);

  const suggestion = suggestForGenre(song?.genre);
  const suggestionExists = !!suggestion && presets.some((p) => p.name === suggestion);
  const activeFx = fxActive(fx);
  const bindValue = bindKind === 'file' ? song?.path : bindKind === 'album' ? song?.album : bindKind === 'genre' ? song?.genre : '';
  const bindLabel = (b?: PresetBind) => !b ? '' : b.kind === 'file' ? '・綁檔案' : b.kind === 'album' ? `・綁專輯 ${b.value}` : `・綁曲風 ${b.value}`;

  const vslider = 'eq-v accent-[#e23b2f]';
  const style = pos
    ? { left: pos.x, top: pos.y }
    : { left: '50%', top: 48, transform: 'translateX(-50%)' };
  const tabBtn = (id: PanelTab, label: string, badge?: string) => (
    <button
      type="button" role="tab" aria-selected={tab === id}
      className={`h-10 px-3 text-[13px] border-b-2 ${tab === id ? 'border-[#e23b2f] text-[#f4f0e8] font-bold' : 'border-transparent text-[#8a847a]'}`}
      onClick={() => setTab(id)}
    >{label}{badge ? <span className="ml-1 text-[10px] text-[#e23b2f]">{badge}</span> : null}</button>
  );

  return (
    <div
      ref={boxRef}
      role="dialog"
      aria-label="音效與等化器"
      className="fixed z-50 bg-[#12110f] border border-[#2a2723] shadow-[0_12px_40px_rgba(0,0,0,0.6)] rounded-sm select-none"
      style={{ ...style, width: 'min(860px, calc(100vw - 24px))', maxHeight: 'calc(100vh - 64px)', overflowY: 'auto' }}
    >
      <div
        className="flex items-center gap-1 px-2 border-b border-[#2a2723] cursor-move sticky top-0 bg-[#12110f] z-10"
        onPointerDown={(e) => {
          if ((e.target as HTMLElement).closest('button,input,select,label')) return;
          const r = boxRef.current?.getBoundingClientRect();
          if (!r) return;
          drag.current = { dx: e.clientX - r.left, dy: e.clientY - r.top };
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const x = Math.max(0, Math.min(window.innerWidth - 120, e.clientX - drag.current.dx));
          const y = Math.max(0, Math.min(window.innerHeight - 40, e.clientY - drag.current.dy));
          setPos({ x, y });
        }}
        onPointerUp={() => { drag.current = null; }}
      >
        <div role="tablist" className="flex">
          {tabBtn('eq', '等化器', enabled ? '' : '關')}
          {tabBtn('fx', '音效', activeFx.length ? String(activeFx.length) : '')}
          {tabBtn('suggest', '建議')}
        </div>
        <span className="ml-2 text-[11px] text-[#8a847a] truncate min-w-0">
          {activeFx.length ? `作用中：${activeFx.join('、')}` : '音效全關'}
        </span>
        <button type="button" className="round ml-auto shrink-0" aria-label="關閉音效面板" title="關閉（Esc）" onClick={onClose}>✕</button>
      </div>

      {tab === 'eq' && (
        <>
          <div className="flex flex-wrap items-center gap-3 px-4 pt-3 text-[12px]">
            <label className="flex items-center gap-2 cursor-pointer">
              <input type="checkbox" checked={enabled} onChange={(e) => onEnabled(e.target.checked)} className="accent-[#e23b2f]" />
              等化器{enabled ? '已啟用' : '已關閉'}
            </label>
            <span className="text-[11px] text-[#8a847a]">18 段 · ±15 dB · 本地檔與電台皆套用 · 雙擊推桿歸零</span>
            <button type="button" className="round w-auto px-3 text-xs ml-auto" onClick={() => { onBands(Array(bands.length).fill(0)); onPreamp(0); }}>歸零</button>
          </div>
          <div className={`px-4 pt-2 ${enabled ? '' : 'opacity-50'}`}>
            <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="w-full h-[88px] block bg-[#0d0c0b] border border-[#2a2723]" aria-label="頻率響應曲線">
              {[-15, 0, 15].map((db) => (
                <line key={db} x1={0} x2={W} y1={yOf(db)} y2={yOf(db)} stroke={db === 0 ? '#3a3631' : '#221f1c'} strokeWidth={1} vectorEffect="non-scaling-stroke" />
              ))}
              {EQ_HZ.map((f) => (
                <line key={f} x1={xOf(f)} x2={xOf(f)} y1={0} y2={H} stroke="#1c1a17" strokeWidth={1} vectorEffect="non-scaling-stroke" />
              ))}
              {path && <path d={path} fill="none" stroke="#e23b2f" strokeWidth={2} vectorEffect="non-scaling-stroke" />}
            </svg>
            <div className="flex justify-between text-[10px] text-[#8a847a] mt-1">
              <span>20 Hz</span>
              <span>{curve.length ? `最高 ${fmt(Math.round(peak * 10) / 10)} dB${peak > 0.5 ? '，過高會削波，可降前級' : ''}` : '曲線不可用'}</span>
              <span>20 kHz</span>
            </div>
          </div>

          <div className={`px-4 pt-2 pb-3 flex gap-2 ${enabled ? '' : 'opacity-50'}`}>
            <div className="flex flex-col items-center gap-1 w-12 shrink-0 border-r border-[#2a2723] pr-2">
              <span className="num text-[10px] text-[#e23b2f] h-4">{fmt(preamp)}</span>
              <input
                aria-label="前級增益" title="前級增益（雙擊歸零）"
                type="range" min={-EQ_LIMIT} max={EQ_LIMIT} step={0.5} value={preamp}
                onChange={(e) => onPreamp(clampDb(e.target.value))}
                onDoubleClick={() => onPreamp(0)}
                className={vslider}
              />
              <span className="text-[10px] text-[#8a847a]">前級</span>
            </div>
            <div className="flex flex-col justify-between text-[9px] text-[#5d5850] h-[150px] mt-5 shrink-0 text-right">
              <span>+15</span><span>0</span><span>−15</span>
            </div>
            <div className="grid flex-1 min-w-0" style={{ gridTemplateColumns: `repeat(${EQ_LABELS.length}, minmax(0, 1fr))` }}>
              {EQ_LABELS.map((label, i) => (
                <div key={label} className="flex flex-col items-center gap-1 min-w-0">
                  <span className="num text-[10px] text-[#e23b2f] h-4">{fmt(bands[i] ?? 0)}</span>
                  <input
                    aria-label={`${label} Hz`} title={`${label} Hz（雙擊歸零）`}
                    type="range" min={-EQ_LIMIT} max={EQ_LIMIT} step={0.5}
                    value={bands[i] ?? 0}
                    onChange={(e) => set(i, Number(e.target.value))}
                    onDoubleClick={() => set(i, 0)}
                    className={vslider}
                  />
                  <span className="text-[10px] text-[#8a847a] whitespace-nowrap">{label}</span>
                </div>
              ))}
            </div>
          </div>

          <div className="px-4 py-2 border-t border-[#2a2723] flex flex-wrap items-center gap-2 text-[12px]">
            <span className="text-[11px] tracking-widest text-[#8a847a]">這首建議</span>
            {song?.genre
              ? suggestion
                ? <>
                    <span>曲風「{song.genre}」→ <b className="text-[#f4f0e8]">{suggestion}</b></span>
                    <button type="button" className="round w-auto px-3 text-xs" disabled={!suggestionExists || selectedPreset === suggestion} onClick={() => onApplyPreset(suggestion)}>
                      {selectedPreset === suggestion ? '已套用' : suggestionExists ? '套用' : '這個預設已刪除'}
                    </button>
                  </>
                : <span className="text-[#8a847a]">曲風「{song.genre}」沒有對應的建議，可存一個預設並綁定這個曲風。</span>
              : <span className="text-[#8a847a]">{song ? '這首沒有曲風標籤，可在「標籤」補上，或綁定檔案／專輯。' : '尚未播放。'}</span>}
          </div>
          <div className="px-4 py-2 border-t border-[#2a2723] flex flex-wrap items-center gap-2 text-[12px]">
            <label className="flex items-center gap-2 cursor-pointer" title="與 參考播放器「依曲目自動調整等化器預設」相同：檔案 → 專輯 → 曲風綁定 → 依曲風建議 → 回退">
              <input type="checkbox" checked={auto.enabled} onChange={(e) => onAuto({ ...auto, enabled: e.target.checked })} className="accent-[#e23b2f]" />
              換歌時自動套用
            </label>
            <span className="text-[#8a847a]">沒有符合時</span>
            <select className="field min-w-[120px]" style={{ flex: '0 1 160px' }} value={auto.fallback} disabled={!auto.enabled} onChange={(e) => onAuto({ ...auto, fallback: e.target.value })}>
              <option value="">維持目前曲線</option>
              {presets.map((p) => (<option key={p.name} value={p.name}>{p.name}</option>))}
            </select>
            {auto.enabled && autoNote && <span className="text-[11px] text-[#e23b2f] truncate min-w-0">{autoNote}</span>}
          </div>

          <div className="px-4 py-3 border-t border-[#2a2723] flex flex-wrap items-center gap-2">
            <span className="text-[11px] tracking-widest text-[#8a847a]">預設</span>
            <select
              className="field min-w-[140px]" style={{ flex: '0 1 220px' }}
              value={selectedPreset ?? ''}
              onChange={(e) => { if (e.target.value) onApplyPreset(e.target.value); }}
            >
              <option value="">{selectedPreset ? '' : '-- 自訂 --'}</option>
              {presets.map((p) => (<option key={p.name} value={p.name}>{p.name}{bindLabel(p.bind)}</option>))}
            </select>
            <button type="button" className="round w-auto px-3 text-xs text-[#e23b2f]" onClick={() => { if (selectedPreset && window.confirm(`刪除預設「${selectedPreset}」？`)) onDeletePreset(selectedPreset); }} disabled={!selectedPreset}>刪除</button>
            <input className="field min-w-[120px]" style={{ flex: '1 1 140px' }} placeholder="預設名稱（同名覆寫）" value={presetDraft} onChange={(e) => onPresetNameChange(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter' && presetDraft.trim()) onSavePreset(bindKind && bindValue ? { kind: bindKind, value: bindValue } : undefined); }} />
            <select className="field" style={{ flex: '0 1 150px' }} value={bindKind} onChange={(e) => setBindKind(e.target.value as '' | PresetBind['kind'])} aria-label="自動套用綁定">
              <option value="">不綁定</option>
              <option value="file" disabled={!song?.path}>綁定這首檔案</option>
              <option value="album" disabled={!song?.album}>綁定這張專輯{song?.album ? `（${song.album}）` : ''}</option>
              <option value="genre" disabled={!song?.genre}>綁定這個曲風{song?.genre ? `（${song.genre}）` : ''}</option>
            </select>
            <button type="button" className="round w-auto px-3 text-xs" onClick={() => onSavePreset(bindKind && bindValue ? { kind: bindKind, value: bindValue } : undefined)} disabled={!presetDraft.trim()}>儲存（含前級）</button>
          </div>
          <p className="px-4 pb-3 text-xs text-[#8a847a]">內建預設與建議是本播放器自調的曲線，不是 參考播放器 內建預設。</p>
        </>
      )}

      {tab === 'fx' && (
        <div className="px-4 py-3">
          <div className="flex items-center gap-2 mb-3 text-[12px]">
            <span className="text-[#8a847a]">每種效果都即時作用；設定跟著工作階段保存。</span>
            <button type="button" className="round w-auto px-3 text-xs ml-auto" onClick={() => onFx({ ...FX_DEFAULT })}>全部關閉並還原</button>
          </div>
          <div className="grid gap-2" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(250px, 1fr))' }}>
            <FxCard title="速度" on={fx.speed !== 1} note={isRadio ? '電台不適用' : '音高跟著變'}>
              <FxSlider label="倍率" value={fx.speed} min={0.5} max={2} step={0.01} fmt={xFmt} onChange={(v) => upd({ speed: v })} disabled={isRadio} />
              <button type="button" className="round w-auto px-2 text-[11px]" disabled={fx.speed === 1} onClick={() => upd({ speed: 1 })}>原速</button>
            </FxCard>
            <FxCard title="節奏" on={fx.tempo !== 1} note={isRadio ? '電台不適用' : '音高不變'}>
              <FxSlider label="倍率" value={fx.tempo} min={0.5} max={2} step={0.01} fmt={xFmt} onChange={(v) => upd({ tempo: v })} disabled={isRadio} />
              <button type="button" className="round w-auto px-2 text-[11px]" disabled={fx.tempo === 1} onClick={() => upd({ tempo: 1 })}>原節奏</button>
            </FxCard>
            <FxCard title="音高" on={fx.pitch !== 0} note={isRadio ? '電台不適用' : '節奏不變'}>
              <FxSlider label="半音" value={fx.pitch} min={-12} max={12} step={0.5} fmt={(v) => `${v > 0 ? '+' : ''}${v}`} onChange={(v) => upd({ pitch: v })} disabled={isRadio} />
              <button type="button" className="round w-auto px-2 text-[11px]" disabled={fx.pitch === 0} onClick={() => upd({ pitch: 0 })}>原調</button>
            </FxCard>
            <FxCard title="低音" on={fx.bassOn} onToggle={(v) => upd({ bassOn: v })} note="低頻擱架">
              <FxSlider label="增益" value={fx.bassDb} min={0} max={15} step={0.5} fmt={(v) => `+${v} dB`} onChange={(v) => upd({ bassDb: v })} disabled={!fx.bassOn} />
              <FxSlider label="頻率" value={fx.bassHz} min={40} max={250} step={5} fmt={(v) => `${v} Hz`} onChange={(v) => upd({ bassHz: v })} disabled={!fx.bassOn} />
            </FxCard>
            <FxCard title="高音增強" on={fx.enhancerOn} onToggle={(v) => upd({ enhancerOn: v })} note="2.5kHz 以上諧波">
              <FxSlider label="強度" value={fx.enhancer} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ enhancer: v })} disabled={!fx.enhancerOn} />
            </FxCard>
            <FxCard title="合唱" on={fx.chorusOn} onToggle={(v) => upd({ chorusOn: v })}>
              <FxSlider label="深度" value={fx.chorusDepth} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ chorusDepth: v })} disabled={!fx.chorusOn} />
              <FxSlider label="速率" value={fx.chorusRate} min={0.1} max={5} step={0.1} fmt={(v) => `${v.toFixed(1)} Hz`} onChange={(v) => upd({ chorusRate: v })} disabled={!fx.chorusOn} />
              <FxSlider label="混合" value={fx.chorusMix} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ chorusMix: v })} disabled={!fx.chorusOn} />
            </FxCard>
            <FxCard title="鑲邊" on={fx.flangerOn} onToggle={(v) => upd({ flangerOn: v })}>
              <FxSlider label="深度" value={fx.flangerDepth} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ flangerDepth: v })} disabled={!fx.flangerOn} />
              <FxSlider label="速率" value={fx.flangerRate} min={0.05} max={2} step={0.05} fmt={(v) => `${v.toFixed(2)} Hz`} onChange={(v) => upd({ flangerRate: v })} disabled={!fx.flangerOn} />
              <FxSlider label="回授" value={fx.flangerFeedback} min={0} max={0.9} step={0.01} fmt={pctFmt} onChange={(v) => upd({ flangerFeedback: v })} disabled={!fx.flangerOn} />
              <FxSlider label="混合" value={fx.flangerMix} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ flangerMix: v })} disabled={!fx.flangerOn} />
            </FxCard>
            <FxCard title="回音" on={fx.echoOn} onToggle={(v) => upd({ echoOn: v })}>
              <FxSlider label="延遲" value={fx.echoMs} min={20} max={2000} step={10} fmt={(v) => `${v} ms`} onChange={(v) => upd({ echoMs: v })} disabled={!fx.echoOn} />
              <FxSlider label="回授" value={fx.echoFeedback} min={0} max={0.9} step={0.01} fmt={pctFmt} onChange={(v) => upd({ echoFeedback: v })} disabled={!fx.echoOn} />
              <FxSlider label="混合" value={fx.echoMix} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ echoMix: v })} disabled={!fx.echoOn} />
            </FxCard>
            <FxCard title="殘響" on={fx.reverbOn} onToggle={(v) => upd({ reverbOn: v })}>
              <FxSlider label="空間" value={fx.reverbRoom} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ reverbRoom: v })} disabled={!fx.reverbOn} />
              <FxSlider label="阻尼" value={fx.reverbDamp} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ reverbDamp: v })} disabled={!fx.reverbOn} />
              <FxSlider label="混合" value={fx.reverbMix} min={0} max={1} step={0.01} fmt={pctFmt} onChange={(v) => upd({ reverbMix: v })} disabled={!fx.reverbOn} />
            </FxCard>
            <FxCard title="人聲消除" on={fx.voiceRemover} onToggle={(v) => upd({ voiceRemover: v })} note="只對立體聲有效">
              <p className="text-[11px] text-[#8a847a]">消掉錄在正中央的聲音（多半是主唱），保留 160Hz 以下的低音。</p>
            </FxCard>
          </div>
          <div className="mt-3 pt-3 border-t border-[#2a2723] flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={normalize} onChange={(e) => onNormalize(e.target.checked)} className="accent-[#e23b2f]" />
              峰值正規化{normalize ? `（目前增益 ${gain.toFixed(2)}）` : ''}
            </label>
            <label className="flex items-center gap-2 flex-1 min-w-[240px]">
              <span className="shrink-0">交叉淡化 {crossfade === 0 ? '關閉' : `${(crossfade / 1000).toFixed(1)} 秒`}</span>
              <input
                type="range" min={0} max={8000} step={200} value={crossfade}
                onChange={(e) => onCrossfade(Number(e.target.value))}
                className="line min-w-0 w-0 flex-1" style={{ ['--pct' as string]: `${(crossfade / 8000) * 100}%` }}
              />
            </label>
            <p className="w-full text-xs text-[#8a847a]">交叉淡化需要下一首已預載，只作用於 15 分鐘以內的本地檔。</p>
          </div>
        </div>
      )}

      {tab === 'suggest' && (
        <div className="px-4 py-3">
          <p className="text-[12px] text-[#8a847a] mb-3">依聆聽情境一鍵套用等化器、前級與音效。套用後仍可在「等化器」「音效」微調。這些是本播放器的建議值，不是 參考播放器 內建。</p>
          <div className="grid gap-2" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(240px, 1fr))' }}>
            {SCENES.map((scene) => {
              const extra = fxActive(sceneFx(scene));
              return (
                <button
                  key={scene.id} type="button"
                  className="text-left border border-[#2a2723] hover:border-[#e23b2f] rounded-sm px-3 py-2"
                  onClick={() => onScene(scene)}
                >
                  <div className="text-[13px] font-bold text-[#f4f0e8]">{scene.name}</div>
                  <div className="text-[11px] text-[#8a847a] mt-1 leading-4">{scene.why}</div>
                  <div className="text-[10px] text-[#e23b2f] mt-1">
                    {scene.id === 'reset' ? '全部歸零' : [`前級 ${fmt(scene.preamp)} dB`, ...extra].join(' · ')}
                  </div>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

export function RadioView({ onPlay }: { onPlay: (url: string) => void }) {
  const [stations, setStations] = useState<RadioStation[]>([]);
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [err, setErr] = useState('');
  const [msg, setMsg] = useState('');
  const [editing, setEditing] = useState<RadioStation | null>(null);
  useEffect(() => { listRadio().then(setStations).catch(() => setStations([])); }, []);
  const clear = () => { setName(''); setUrl(''); setEditing(null); setErr(''); setMsg(''); };
  const deleteStation = async (s: RadioStation) => {
    if (!window.confirm('刪除「' + s.name + '」？')) return;
    try {
      await deleteRadio(s.id);
      setMsg('已刪除');
      clear();
      setStations(await listRadio());
    } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
  };
  return (
    <div className="p-6 max-w-xl">
      <h2 className="text-[26px] font-bold">網路電台</h2>
      <p className="text-sm text-[#8a847a] mt-2 mb-4">只有你自己存過的網址，沒有內建電台目錄。這台電腦要有 ffmpeg 才播得了。</p>
      <form className="space-y-2 mb-4" onSubmit={async (e) => {
        e.preventDefault();
        setErr(''); setMsg('');
        try {
          if (editing) {
            await renameRadio(editing.id, name);
            setMsg('已改名');
          } else {
            const id = await saveRadio(name, url);
            setMsg('已存入電台');
          }
          clear();
          setStations(await listRadio());
        } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
      }}>
        <input className="field" placeholder="電台名稱" value={name} onChange={(e) => setName(e.target.value)} disabled={!!editing} />
        <input className="field" placeholder="https://example.com/stream.mp3" value={url} onChange={(e) => setUrl(e.target.value)} />
        <button className="round w-auto px-4 text-sm" type="submit">{editing ? '儲存改名' : '存入電台'}</button>
        <button type="button" className="text-xs text-[#8a847a]" onClick={clear}>取消</button>
      </form>
      <ul className="space-y-1">
        {stations.map((s) => (
          <li key={s.id} className="flex items-center gap-2 border-b border-[#2a2723] py-2">
            <span className="truncate">{s.name}</span>
            <span className="text-xs text-[#6f6a63] truncate flex-1">{s.url}</span>
            <button className="round w-6 h-6 px-0" onClick={() => { onPlay(s.url); clear(); }} title="播放">▶</button>
            <button className="round w-6 h-6 px-0" onClick={() => { setEditing(s); setName(s.name); setUrl(s.url); }} title="改名">✎</button>
            <button className="round w-6 h-6 px-0 text-[#e23b2f]" onClick={() => { deleteStation(s); }} title="刪除">×</button>
          </li>
        ))}
        {stations.length === 0 && <li className="text-xs text-[#8a847a]">還沒有電台。貼上你自己的串流網址。</li>}
      </ul>
      <div className="mt-4 text-sm text-[#8a847a]">貼上 http / https 串流網址，存進來後就能在這裡播放。</div>
      {err && <p className="text-sm text-[#e23b2f] mt-3">{err}</p>}
      {msg && <p className="text-sm text-[#e23b2f] mt-3">{msg}</p>}
    </div>
  );
}

export function TagView({ song }: { song: Song | null }) {
  const [form, setForm] = useState<TagEdit | null>(null);
  const [msg, setMsg] = useState('');
  useEffect(() => {
    if (!song) { setForm(null); return; }
    readTags(song.id).then(setForm).catch((e) => setMsg(String(e)));
  }, [song?.id]);
  if (!song || !form) return <div className="p-6 text-sm text-[#8a847a]">先在清單選一首歌，再改標籤。</div>;
  const set = (k: keyof TagEdit, v: string) => setForm({ ...form, [k]: (k === 'year' || k === 'track_number' || k === 'disc_number') ? (v ? Number(v) : null) : v });
  return (
    <form className="p-6 max-w-xl space-y-3" onSubmit={async (e) => {
      e.preventDefault();
      setMsg('');
      try { await writeTags(form); setMsg('已寫入檔案與資料庫'); }
      catch (err) { setMsg(err instanceof Error ? err.message : String(err)); }
    }}>
      <h2 className="text-[26px] font-bold">標籤</h2>
      <p className="text-sm text-[#8a847a]">{song.path}</p>
      {(['title', 'artist', 'album', 'genre'] as const).map((k) => (
        <label key={k} className="block text-sm">{k === 'title' ? '歌名' : k === 'artist' ? '歌手' : k === 'album' ? '專輯' : '曲風'}
          <input className="field mt-1" value={String(form[k] ?? '')} onChange={(e) => set(k, e.target.value)} />
        </label>
      ))}
      <label className="block text-sm">年份
        <input className="field mt-1" value={form.year ?? ''} onChange={(e) => set('year', e.target.value)} />
      </label>
      <label className="block text-sm">音軌號
        <input className="field mt-1" value={form.track_number ?? ''} onChange={(e) => set('track_number', e.target.value)} />
      </label>
      <label className="block text-sm">碟號
        <input className="field mt-1" value={form.disc_number ?? ''} onChange={(e) => set('disc_number', e.target.value)} />
      </label>
      <button className="round w-auto px-4 text-sm" type="submit">寫入標籤</button>
      {msg && <p className="text-sm text-[#e23b2f]">{msg}</p>}
    </form>
  );
}

export function ConvertView({ song }: { song: Song | null }) {
  const [ok, setOk] = useState<boolean | null>(null);
  const [format, setFormat] = useState('mp3');
  const [bitrate, setBitrate] = useState('192');
  const [dir, setDir] = useState('');
  const [msg, setMsg] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => { ffmpegAvailable().then(setOk).catch(() => setOk(false)); }, []);
  if (ok === false) return <div className="p-6 text-sm">這台電腦找不到 ffmpeg，無法轉檔。裝好並放進 PATH 後再打開這個畫面。</div>;
  return (
    <div className="p-6 max-w-xl space-y-3">
      <h2 className="text-[26px] font-bold">轉檔</h2>
      <p className="text-sm text-[#8a847a]">{song ? song.path : '先選一首歌。'} 轉完才會顯示輸出路徑，沒有進度條。</p>
      <select className="field" value={format} onChange={(e) => setFormat(e.target.value)}>
        <option value="mp3">mp3</option>
        <option value="flac">flac</option>
        <option value="wav">wav</option>
        <option value="ogg">ogg</option>
      </select>
      {format === 'mp3' && (
        <select className="field" style={{ width: 120 }} value={bitrate} onChange={(e) => setBitrate(e.target.value)}>
          <option value="128">128 kbps</option>
          <option value="192">192 kbps</option>
          <option value="320">320 kbps</option>
        </select>
      )}
      <input className="field" placeholder="輸出資料夾，例如 D:\\Out" value={dir} onChange={(e) => setDir(e.target.value)} />
      <button
        className="round w-auto px-4 text-sm" disabled={!song || busy || ok === null}
        onClick={async () => {
          if (!song) return;
          setBusy(true); setMsg('');
          try { setMsg(await convertAudio({ song_id: song.id, format, output_dir: dir, bitrate_kbps: Number(bitrate) })); }
          catch (e) { setMsg(e instanceof Error ? e.message : String(e)); }
          finally { setBusy(false); }
        }}
      >{busy ? '轉檔中' : '開始轉檔'}</button>
      {msg && <p className="text-sm break-all">{msg}</p>}
    </div>
  );
}

export function SettingsView({ onScanned }: { onScanned: () => void }) {
  const [folders, setFolders] = useState<string[]>([]);
  const [draft, setDraft] = useState('');
  const [msg, setMsg] = useState('');
  const reload = async () => {
    const fs = await listMusicFolders();
    setFolders(fs.map((f) => f.path));
  };
  useEffect(() => { reload().catch((e) => setMsg(String(e))); }, []);
  return (
    <div className="p-6 max-w-xl space-y-3">
      <h2 className="text-[26px] font-bold">音樂資料夾</h2>
      <p className="text-sm text-[#8a847a]">選擇資料夾，或直接貼路徑。取消選擇不會改動已加入的資料夾。</p>
      <div className="flex gap-2">
        <input className="field" placeholder="D:\\Music" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <button type="button" className="round w-auto px-3 text-sm" onClick={async () => {
          try {
            const picked = await browseFolder();
            if (!picked) return;
            setDraft(picked);
          } catch (e) {
            setMsg(e instanceof Error ? e.message : String(e));
          }
        }}>選擇</button>
        <button className="round w-auto px-3 text-sm" onClick={async () => {
          if (!draft.trim()) return;
          await addMusicFolder({ path: draft.trim() });
          setDraft('');
          await reload();
        }}>加入</button>
      </div>
      <ul className="text-sm space-y-1">
        {folders.map((f) => (
          <li key={f} className="flex justify-between gap-3 border-b border-[#2a2723] py-1">
            <span className="truncate">{f}</span>
            <button className="text-[#e23b2f]" onClick={async () => { await removeMusicFolder(f); await reload(); }}>移除</button>
          </li>
        ))}
      </ul>
      <button className="round w-auto px-4 text-sm" disabled={folders.length === 0} onClick={async () => {
        setMsg('掃描中');
        try { await scanFolders(folders); setMsg('掃描已送出'); onScanned(); }
        catch (e) { setMsg(e instanceof Error ? e.message : String(e)); }
      }}>掃描</button>
      {msg && <p className="text-sm text-[#8a847a]">{msg}</p>}
    </div>
  );
}
