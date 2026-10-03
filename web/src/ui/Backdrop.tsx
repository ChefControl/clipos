// The page ground: a poster blurred into colour behind the top of the page, fading into
// the dark (the clip page uses the clip's own poster). Without an image, a faint amber
// glow. Purely decorative and fixed behind everything.
export function Backdrop({ image }: { image?: string | null }) {
  return (
    <div aria-hidden="true" className="pointer-events-none fixed inset-0 -z-10 overflow-hidden">
      {image && (
        <img
          src={image}
          alt=""
          className="absolute -inset-x-16 top-0 h-[70vh] w-[calc(100%+8rem)] object-cover opacity-90 blur-[80px] brightness-[.45] saturate-[1.4]"
        />
      )}
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_40%_30%_at_18%_14%,rgba(245,165,36,0.14),rgba(245,165,36,0)_100%),linear-gradient(180deg,rgba(5,5,6,0.55)_0,rgba(5,5,6,0)_10%,rgba(5,5,6,0.3)_30%,#050506_60%)]" />
    </div>
  );
}
