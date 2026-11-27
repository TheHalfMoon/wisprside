const BRAND_NAME = "WisprSide";

const HandyTextLogo = ({
  width = 124,
  height = 34,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height}
      className={className}
      viewBox="0 0 240 56"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-label="WisprSide"
      role="img"
    >
      <rect
        x="4"
        y="12"
        width="6"
        height="32"
        rx="3"
        className="logo-primary"
      />
      <rect
        x="14"
        y="6"
        width="6"
        height="44"
        rx="3"
        className="logo-primary"
      />
      <rect
        x="24"
        y="16"
        width="6"
        height="24"
        rx="3"
        className="logo-primary"
      />
      <text
        x="40"
        y="37"
        className="logo-primary"
        style={{
          fontFamily:
            "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, sans-serif",
          fontSize: "28px",
          fontWeight: 700,
          letterSpacing: "-0.03em",
        }}
      >
        {BRAND_NAME}
      </text>
    </svg>
  );
};

export default HandyTextLogo;
