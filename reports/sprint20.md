# Sprint 20 — Diagnóstico de JavaScript e atualização do DOM

Captura final: `reports/google_s20.png` (comparar com `google_s19.png`).

## Causas confirmadas

| # | Evidência | Causa |
|---|---|---|
| 1 | `<script src="…">` do Google nunca executava: 0 scripts externos no dump, e o teste local `19-extscript.html` mostrava `ext carregado: false` | `set_html_with_images` só absolutizava `<img src>`; `<script src>` e `<link href>` ficavam relativos, então o payload pré-baixado (chaveado por URL absoluta) nunca casava |
| 2 | `file://` não carregava scripts externos | `local_images()` só lia `<img>` |
| 3 | `function` declarada em um script não ficava visível no seguinte | Cada script era embrulhado em `try { … } catch`, o que torna declarações de função **block-scoped** |
| 4 | Os três `not a function` do Google | O `catch` interno do Closure chama `_._DumpException`, que não existia; o erro secundário mascarava o original |

## Correções implementadas

| Frente | Arquivo | Detalhe |
|---|---|---|
| **URLs de recursos** | `naro-engine` | Absolutiza `img[src]`, `script[src]` e `link[href]` antes de casar com os payloads |
| **Scripts em `file://`** | `naro-engine` | `local_images()` também lê `<script src>` |
| **Escopo global de scripts** | `naro-js` | `__naro_eval` usa **eval indireto** `(0, eval)(src)`: funções ficam globais e o erro real é reportado |
| **DOM API** | `naro-js`, `naro-html` | `classList` (add/remove/toggle/contains/length/item), `element.style` (Proxy com `setProperty`/`getPropertyValue`/`removeProperty` e atribuição direta), `insertBefore` + `attach_before` |

## Testes

`cargo test --workspace`: **60 passando, 0 falhas**.
`naro --compat tests/compat`: **19 PASS, 0 FAIL, 1 UNSUP (total 20)**, com dois casos novos:

- `19-extscript.html` — script externo define função global e o script seguinte a chama; verifica `log:ext carregado: true` e o texto renderizado.
- `20-domapi.html` — `classList.add/contains/remove`, `style.setProperty` refletido em `bg=#eef0f4` no layout, `insertBefore` contando 1 filho.

## Captura antes/depois

| Aspecto | `google_s19.png` | `google_s20.png` |
|---|---|---|
| Erros JS | 3 × `not a function` | **0** |
| Logo / botões / rodapé | corretos | idênticos |
| Campo de pesquisa | visível, retangular | **idêntico** |
| Ícone do teclado | fora do campo | **idêntico** |

**A página não mudou visualmente.** Isso é o resultado honesto: os erros
desapareceram, mas o Google não aplicou os estilos do campo.

## Por que o campo continua sem estilo

O Google não usa `<script src>` nesta página (0 ocorrências); os scripts são
inline e agora rodam sem erro. O que falta é outra coisa:

- o CSS que dá `border-radius`, sombra e fundo ao campo vive em **`@media`**,
  que o parser descarta (não avalia) em vez de aplicar;
- as classes que posicionam o wrapper (`position:relative`) são aplicadas por
  JS **depois** de `DOMContentLoaded`, em um callback que depende de
  `requestAnimationFrame` + `MutationObserver` — este último ausente.

Ou seja: o bloqueio deixou de ser "script quebrado" e passou a ser
**`@media` não avaliado** e **`MutationObserver` ausente**. Ambos estão fora do
escopo desta sprint.

## Erros JavaScript restantes

No Google: **nenhum**. No MDN: um (`not a function @ <input>:5:11`), não
investigado nesta sprint.

## Limitações

- `@media` é descartado no parser: nenhuma regra responsiva se aplica.
- `MutationObserver`, `position: sticky`, `display: contents`, Shadow DOM
  continuam ausentes.
- O MDN mantém 1 erro JS.

## Disco

12 GB livres antes e depois. Três builds de release; nenhum diretório removido.
