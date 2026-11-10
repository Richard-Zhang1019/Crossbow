import { forwardRef, type ButtonHTMLAttributes } from "react";
import "./Button.css";

export type ButtonVariant = "secondary" | "primary";
export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  context?: "content" | "toolbar";
  size?: "default" | "small";
  tone?: "neutral" | "danger";
  iconOnly?: boolean;
  loading?: boolean;
};

/** Semantic foundation for navigation, switches and other controls with their own shape.
 * Keep the native button/ref/events, disable implicit form submission, share busy behavior.
 */
export const ButtonBase = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { loading?: boolean }
>(function ButtonBase(
  { type = "button", disabled, loading = false, className, ...rest },
  ref,
) {
  return (
    <button
      {...rest}
      ref={ref}
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || rest["aria-busy"] || undefined}
      className={["cb-button-base", className].filter(Boolean).join(" ")}
    />
  );
});

/** Two action styles, with independent toolbar/content context and semantic danger tone. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  function Button(
    {
      variant = "secondary",
      context = "content",
      size = "default",
      tone = "neutral",
      iconOnly = false,
      loading = false,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    return (
      <ButtonBase
        {...rest}
        ref={ref}
        loading={loading}
        className={["cb-button", className].filter(Boolean).join(" ")}
        data-variant={variant}
        data-context={context}
        data-size={size}
        data-tone={tone}
        data-icon-only={iconOnly || undefined}
        data-loading={loading || undefined}
      >
        <span className="cb-button-label">{children}</span>
        {loading && (
          <svg
            className="cb-button-spinner"
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            aria-hidden="true"
          >
            <circle
              cx="12"
              cy="12"
              r="9"
              stroke="currentColor"
              strokeWidth="2"
              opacity=".2"
            />
            <path
              d="M12 3a9 9 0 0 1 9 9"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
            />
          </svg>
        )}
      </ButtonBase>
    );
  },
);
