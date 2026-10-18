# Sprint 24 — Fase 2: unidades e funções CSS

Escopo: Fase 2 do plano mestre. Fase 1 preservada (nenhuma alteração de seletor ou
cascata além de passar contexto para a resolução de comprimentos).

## 1. Arquivos modificados

| Arquivo | Responsabilidade da mudança |
|---|---|
| `crates/naro-css/src/lib.rs` | `Dim` virou enum (`Linear{px,percent}`, `Min`, `Max`, `Clamp`); `LengthCtx`/`StyleContext`; parser de `<length>` com `px`/`em`/`rem`/`%`/`vw`/`vh`/`vmin`/`vmax` e `calc()`/`min()`/`max()`/`clamp()`; `EdgesDim` para `margin`/`padding`; `Track.percent`; duas passadas da cascata (fonte, depois o resto); `line-height` com unidades |
| `crates/naro-layout/src/lib.rs` | Resolve `Dim`/`EdgesDim` no contexto certo: largura do bloco contentor para `width`/`min`/`max-width`, `margin`, `padding` e colunas de grid; altura definida só quando não depende de um contentor de altura automática; `top`/`bottom` relativos deixam de usar a largura |
| `crates/naro-engine/src/lib.rs` | `compute_styles_with(doc, sheet, StyleContext)` com viewport real (largura e altura) e base de `rem`; `--bench` idem |
| `crates/naro-app/src/main.rs` | Check `blockw:<tag>:<W>` no runner de compatibilidade |
| `tests/compat/24-units.html`, `manifest.txt` | Página da fase com `calc`, `min`, `clamp`, `vw`, `vh`, `rem`, `em` e cores distintas por bloco |
| `README.md` | Unidades/funções suportadas e limitações reais |

## 2. Unidades e funções

| Item | Antes | Agora |
|---|---|---|
| `px` | ok | ok |
| `%` (width/height/min/max/offsets/grid) | ok (height usava a **largura** como base) | preservado como `Dim::Linear{percent}`; `height` sem contentor definido resolve como `auto` |
| `%` em `margin`/`padding` | descartado em silêncio | `EdgesDim`, resolve contra a largura do bloco contentor (inclusive lados verticais) |
| `em` | só em propriedades de comprimento, usando `font-size` já computado | `font-size`/`font` usam o pai; demais propriedades usam o próprio elemento; ordem da declaração não importa (duas passadas) |
| `rem` | **quebrado**: `3rem` era lido como `em` (sufixo `em` casava primeiro) e falhava o parse → regra descartada | usa o `font-size` computado de `html`, com 16px como valor inicial |
| `vw`/`vh`/`vmin`/`vmax` | ausentes | resolvidos na cascata com a viewport real da engine |
| `calc()` | ausente | `+ - * /`, parênteses, precedência; soma/subtração só entre tipos compatíveis; multiplicação exige um número; divisão por zero é inválida |
| `min()`/`max()`/`clamp()` | ausentes | dobrados para px quando todos os argumentos são absolutos; preservados como árvore quando há percentual (a ordem muda com a base) |
| `line-height` | só número e `px` | número, `px`, `em`, `rem`, `%` |

`Dim::resolve(base)` é o único ponto de resolução; nada é convertido para px "como
se fosse px". Percentual nunca é resolvido no parse.

## 3. Testes

`cargo test --workspace` → **110 testes, 0 falhas** (87 na Sprint 23; +23).

Novos em `naro-css` (13):

| Teste | Cobre |
|---|---|
| `absolute_units_resolve_against_viewport_and_root` | `rem`, `vw`, `vh`, `vmin`, `vmax`, duas viewports |
| `rem_follows_root_font_size` | `rem` de descendente segue `html{font-size}`; `rem` no próprio `html` usa 16 |
| `em_in_font_size_uses_parent` | `1.5em` e `150%` em `font-size` |
| `em_in_lengths_uses_own_font_size` | `padding:2em` declarado antes de `font-size:20px` |
| `calc_addition_and_subtraction` | `calc(100% - 32px)` com duas bases, `calc(50vw - 10px)`, `em` dentro de `calc` |
| `calc_multiplication_and_division` | `2 * 10px`, `100px / 4`, `100% / 4` |
| `calc_precedence_and_parentheses` | `2px + 3px * 4`, `(2px + 3px) * 4`, `100% - (10px + 20px)` |
| `min_max_clamp_functions` | `min(800px,90%)` e `max(320px,50%)` com duas bases, `clamp(320px,80vw,1200px)`, `clamp()` em `font-size` |
| `percentages_stay_symbolic_until_layout` | `absolute()` é `None`; `resolve(400)`/`resolve(800)` |
| `invalid_lengths_do_not_break_the_sheet` | `10furlong`, `calc(10px*10px)`, `calc(10px+5)`, `min()`, `calc(10px/0)`, `10`, `padding:10px furlong` |
| `line_height_accepts_units_and_numbers` | `1.5`, `30px`, `1.5em`, `150%` produzem 1.5 |
| `px_and_percent_keep_working` | `100px`, `25%`, `padding:4px 8px`, `margin:2px` |
| `grid_percentage_tracks_keep_their_percent` | `25% 75%` guarda percentual, não px |

Novos em `naro-layout` (10):

| Teste | Cobre |
|---|---|
| `percentage_widths_resolve_against_the_container` | `width:50%` → 200 de 400 |
| `calc_percentage_minus_pixels_reaches_layout` | `calc(100% - 32px)` → 368 |
| `min_max_clamp_produce_real_widths` | `min(800px,90%)` → 360, `max(320px,50%)` → 320 |
| `clamp_uses_the_viewport_for_vw` | `clamp(320px,80vw,1200px)` → 800 |
| `rem_padding_uses_the_root_font_size` | `padding:1rem` idêntico a `20px` com raiz 20px, e diferente de 16px |
| `grid_percentage_columns_use_the_container_width` | colunas 100/300 de 400 |
| `vh_sets_a_real_height` | `height:10vh` → 70 |
| `vh_inside_calc_reaches_layout` | `calc(20vh - 40px)` → 100 |
| `percentage_padding_and_margin_use_the_container_width` | `padding:10% 5%; margin:5%` |
| `percentage_height_against_auto_height_is_auto` | `height:50%` não vira metade da largura |

Compatibilidade: `naro --compat tests/compat` → **23 pass, 0 falhas, 1 unsup (24)**;
era 22/0/1. `24-units.html` mede geometria real:

```
section  calc(100% - 200px)        w=800   bg=#0a7d3f
article  min(800px, 50%)           w=500   bg=#8a1f1f
aside    clamp(300px, 80vw, 500px) w=500   bg=#681da8
nav      50vw                      w=500   bg=#1558d6
main     10vh                      h=70    bg=#0e7490
header   2rem                      h=40    bg=#b45309
.card    calc(50vw - 100px)        w=400   bg=#334155
footer   padding:1rem; margin:2em
```

Render: `reports/s24-units.png` (a partir de `s24-units.bmp`) — os oito blocos
aparecem com as cores e dimensões declaradas, conferidas pixel a pixel.

## 4. Baseline × depois (Sprint 23 → Sprint 24)

| Página | STATUS | CONTENT_HEIGHT | JS ERROR | Textos |
|---|---|---|---|---|
| `naro://start` | Idle → Idle | 606,40 → 606,40 | nenhum | idênticos |
| `example.com` | Idle → Idle | 172,80 → **315,80** | `s.js` pré-existente | idênticos |
| `rust-lang.org` | Idle → Idle | 3233,16 → **4338,38** | nenhum | nenhum texto perdido |
| `google.com` | Idle → Idle | 484,00 → 480,33 | nenhum | idênticos |
| MDN `/Web/HTML` | Idle → Idle | 11322,00 → **12039,00** | 6 = 6 | nenhum texto perdido |

Nenhum `STATUS: Error`, nenhum `JS ERROR` novo, nenhum texto ausente (checagem por
substring nos runs de texto).

Causas verificadas, todas por regra da própria página:

- **example.com** (`body{padding:25vh 2em 2em}`): antes `25vh` era descartado e o
  padding caía para `2em 2em`; agora o topo é 175px, igual ao navegador. O bloco
  `body` também mostra a correção de `em` em padding (2em = 32px, largura 480 =
  26em + 2×2em).
- **rust-lang.org**: toda a tipografia do Tachyons é em `rem`
  (`font-size:3rem` no `h1`, `.f2{2.25rem}` no hero). Antes o parser lia `3rem` como
  `em` (o sufixo `em` casava primeiro), o parse falhava e a regra sumia — o `h1`
  ficava com os 32px do UA. Agora o `h1` tem 48px, o hero 36px, como no navegador.
  O aumento de altura é maior que o de um navegador porque a barra de navegação
  continua sendo uma coluna estreita (limitação de dimensionamento de item flex,
  item da Fase 5), e cada item agora ocupa duas linhas.
- **google.com**: −3,67px em blocos de rodapé; `font-size:83%` agora é resolvido em
  vez de descartado.
- **MDN**: +6% porque `rem`/`%` de tipografia passam a aplicar.

`--bench` (`naro://start`): parse 0,017ms, style 0,24–0,31ms, layout 0,05–0,07ms,
paint 0,50ms. A cascata agora faz duas passadas sobre as mesmas declarações; o custo
é sub-milissegundo nesta página. Não há baseline de bench da Sprint 23 para comparar
mediana; a otimização é escopo da Fase 7.

## 5. Limitações remanescentes

- `height: %` contra contentor de altura automática resolve como `auto` (a engine não
  rastreia altura definida do pai no fluxo de blocos). Posicionamento absoluto usa a
  altura da área contentora e é correto.
- `top`/`bottom` em `position: relative` com percentual resolvem para 0, pela mesma
  razão.
- `border-radius: 50%` é ignorado (só px/em/rem).
- Sem `pt`, `pc`, `ch`, `ex`, `lh`, `dvh`/`svh`/`lvh`, `calc()` com `var()` já
  substituído funciona, mas `calc()` aninhado dentro de `min()`/`clamp()` só como
  argumento direto.
- `margin: 10% auto` mantém apenas o primeiro valor numérico para o eixo vertical;
  `auto` vertical continua não implementado.
- Grid `fr` + percentual na mesma trilha soma as duas contribuições (o percentual é
  tratado como base fixa).

## 6. Roadmap

Fases 3–7 intactas. Próxima é a Fase 3 (estado do documento e invalidação).
