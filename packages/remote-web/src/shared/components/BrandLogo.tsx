interface BrandLogoProps {
  className?: string;
  alt?: string;
}

export function BrandLogo({
  className = "h-8 w-auto",
  alt = "Agent Deck",
}: BrandLogoProps) {
  return (
    <picture>
      <source
        srcSet="/agent-deck-logo-dark.svg"
        media="(prefers-color-scheme: dark)"
      />
      <img src="/agent-deck-logo.svg" alt={alt} className={className} />
    </picture>
  );
}
