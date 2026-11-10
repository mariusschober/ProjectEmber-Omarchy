pub const CSS: &str = r#"
window.ember-window,
scrolledwindow {
  background: transparent;
}

scrollbar {
  background: transparent;
}

scrollbar slider {
  background: #4a4442;
  border-radius: 4px;
  min-width: 4px;
  min-height: 24px;
}

.ember-root {
  background: transparent;
}
.ember-panel {
  background-color: #171719;
  border-radius: 18px;
  border: 1px solid rgba(255,255,255,0.075);
  box-shadow: 0 18px 60px rgba(0,0,0,0.55);
}
.ember-wash {
  background-image: linear-gradient(160deg, rgba(74,30,22,0.55), rgba(23,23,25,0.02) 46%, rgba(23,23,25,0.0));
  border-radius: 18px;
}
.ember-header-title {
  color: #f6f0ec;
  font-size: 13px;
  font-weight: 800;
  letter-spacing: 2.4px;
}
.ember-header-subtitle {
  color: #9a908b;
  font-size: 8.5px;
  font-weight: 600;
  letter-spacing: 1.6px;
}
.ember-version {
  color: #6f6663;
  font-size: 9px;
  font-weight: 600;
  background-color: rgba(255,255,255,0.05);
  border-radius: 8px;
  padding: 2px 7px;
}
.ember-hero {
  background-image: linear-gradient(135deg, #47241e, #331d19 55%, #2a1a17);
  border-radius: 15px;
  border: 1px solid rgba(255,77,46,0.14);
}
.ember-hero-title {
  color: #f8f2ee;
  font-size: 15px;
  font-weight: 700;
}
.ember-hero-detail {
  color: #cfc3bd;
  font-size: 11px;
}
.ember-hero-meta {
  color: #e08a72;
  font-size: 9.5px;
  font-weight: 600;
}
.ember-section {
  color: #a29792;
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 1.8px;
}
.ember-card {
  background-color: #272627;
  border-radius: 14px;
  border: 1px solid rgba(255,255,255,0.055);
}
.ember-row {
  padding: 10px 14px;
}
.ember-row-title {
  color: #f5f0ed;
  font-size: 12px;
  font-weight: 600;
}
.ember-row-detail {
  color: #a79d99;
  font-size: 10px;
}
.ember-row-caption {
  color: #8b817d;
  font-size: 9px;
}
.ember-row-icon {
  color: #d9cdc7;
}
.ember-separator {
  background-color: rgba(255,255,255,0.055);
  min-height: 1px;
}
.ember-pill {
  background-color: #222122;
  border-radius: 20px;
  padding: 4px;
  border: 1px solid rgba(255,255,255,0.05);
}
.ember-pill button {
  background: none;
  background-image: none;
  border: none;
  box-shadow: none;
  color: #b9aeaa;
  font-size: 12px;
  font-weight: 600;
  border-radius: 16px;
  padding: 7px 4px;
  min-height: 26px;
}
.ember-pill button:hover {
  color: #f1e8e3;
  background-color: rgba(255,255,255,0.045);
}
.ember-pill button.selected {
  color: #fff7f3;
  background-image: linear-gradient(160deg, #d9472a, #a82e18);
  box-shadow: 0 4px 12px rgba(180,50,25,0.35);
}
.ember-slider-title {
  color: #c8bcb6;
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 1.2px;
}
.ember-slider-value {
  color: #d9cec8;
  font-size: 10.5px;
  font-weight: 600;
}
scale.warmth slider {
  background-color: #fff3ec;
  border: 2px solid #ff5a38;
  box-shadow: 0 2px 6px rgba(0,0,0,0.35);
  min-width: 16px;
  min-height: 16px;
  border-radius: 10px;
}
scale.warmth trough {
  min-height: 4px;
  border-radius: 3px;
  background-image: linear-gradient(to right, #6e6866, #b76a3e 45%, #ff7a2f 74%, #ff2f1f);
}
scale.brightness slider {
  background-color: #fff3ec;
  border: 2px solid #e04527;
  box-shadow: 0 2px 6px rgba(0,0,0,0.35);
  min-width: 16px;
  min-height: 16px;
  border-radius: 10px;
}
scale.brightness trough {
  min-height: 4px;
  border-radius: 3px;
  background-image: linear-gradient(to right, #4a4646, #8e3a24 55%, #ff4a2e);
}
scale > trough > highlight,
scale > trough > fill,
scale highlight {
  background-color: transparent;
  background-image: none;
  box-shadow: none;
  min-width: 0;
  min-height: 0;
}
scale > trough {
  border: none;
  box-shadow: none;
}
scale > slider {
  border: none;
  box-shadow: none;
}
.ember-link,
.ember-link label,
button.ember-link label {
  color: #d98972;
  text-decoration: none;
}
.ember-link:hover,
.ember-link:hover label {
  color: #ff9d80;
}
switch.ember-switch {
  background-color: #4a4442;
  border-radius: 13px;
  min-width: 42px;
  min-height: 24px;
  border: none;
  box-shadow: inset 0 1px 2px rgba(0,0,0,0.35);
}
switch.ember-switch:checked {
  background-image: linear-gradient(160deg, #f3552f, #c73518);
}
switch.ember-switch slider {
  background-color: #f7efe9;
  border-radius: 10px;
  min-width: 20px;
  min-height: 20px;
  margin: 2px;
  box-shadow: 0 1px 3px rgba(0,0,0,0.45);
}
switch.ember-switch:disabled {
  opacity: 0.4;
}
.ember-action {
  background: none;
  background-image: none;
  border: 1px solid rgba(255,255,255,0.08);
  border-radius: 10px;
  color: #d7cbc5;
  font-size: 10.5px;
  font-weight: 600;
  padding: 6px 12px;
  box-shadow: none;
}
.ember-action:hover {
  background-color: rgba(255,255,255,0.055);
  color: #fff4ef;
}
.ember-link {
  color: #d98972;
  font-size: 9.5px;
  text-decoration: none;
  background: none;
  border: none;
  padding: 0;
}
.ember-link:hover {
  color: #ff9d80;
}
.ember-footer {
  color: #6f6663;
  font-size: 8.5px;
}
.ember-badge {
  color: #c77861;
  background-color: rgba(255,74,46,0.10);
  border: 1px solid rgba(255,74,46,0.18);
  border-radius: 7px;
  font-size: 7.5px;
  font-weight: 800;
  letter-spacing: 1px;
  padding: 2px 6px;
}
.ember-attention {
  background-color: rgba(255,74,46,0.09);
  border: 1px solid rgba(255,74,46,0.20);
  border-radius: 11px;
  padding: 8px 12px;
}
.ember-attention-title {
  color: #ffb39d;
  font-size: 10px;
  font-weight: 700;
}
.ember-attention-message {
  color: #d9b6ab;
  font-size: 9px;
}
.ember-row-button {
  background: none;
  background-image: none;
  border: none;
  box-shadow: none;
  color: #e08a72;
  font-size: 9.5px;
  font-weight: 600;
  padding: 0 2px;
}
.ember-row-button:hover {
  color: #ffab8f;
}
.ember-diagnostics {
  background-color: #171719;
  color: #e8ddd7;
}
.ember-diagnostics textview {
  background-color: #1e1d1f;
  color: #ddd2cc;
  font-family: monospace;
  font-size: 10px;
  border-radius: 10px;
  padding: 8px;
}
"#;
