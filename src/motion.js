/* 换标签与换档共用的一小段节奏（DESIGN §8：过渡 ≤ 250ms，只动 opacity）。
   减少动效时等待归零；CSS 那边的 transition 也已由媒体查询关掉。 */

const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

const motionMs = () => (reduceMotion.matches ? 0 : 100);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// 窗口改完尺寸不等于已经画好：等两帧，免得新内容在旧尺寸上先露一下。
const afterPaint = () => new Promise((resolve) =>
  requestAnimationFrame(() => requestAnimationFrame(resolve)));
