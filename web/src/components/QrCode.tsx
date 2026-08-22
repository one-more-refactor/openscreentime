// A QR code rendered client-side as inline SVG. Used for the per-device
// parent code (an otpauth:// URI): the secret never leaves the page as an
// image request, and the SVG inherits the theme's ink colour.
import { useEffect, useState } from "react";
import QRCode from "qrcode";

interface Props {
  value: string;
  /** CSS size of the square. */
  size?: number | string;
  label?: string;
}

export function QrCode({ value, size = 176, label }: Props) {
  const [svg, setSvg] = useState<string>("");
  useEffect(() => {
    let alive = true;
    QRCode.toString(value, {
      type: "svg",
      margin: 1,
      errorCorrectionLevel: "M",
      color: { dark: "#000000", light: "#ffffff" },
    })
      .then((s) => {
        if (alive) setSvg(s);
      })
      .catch(() => {
        if (alive) setSvg("");
      });
    return () => {
      alive = false;
    };
  }, [value]);

  return (
    <span
      className="qr"
      role="img"
      aria-label={label ?? "QR code"}
      style={{ width: size, height: size }}
      // The SVG comes from the qrcode library, built from our own string — not
      // user-supplied markup.
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
