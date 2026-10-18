# Sprint 30 — Barra de título integrada, abas e sistema de ícones

Escopo: interface gráfica da janela (`naro-app` + uma primitiva nova em
`naro-paint`). O motor de renderização, o JavaScript, a rede e o layout não foram
tocados.

## 0. Correção de premissa

O pedido descrevia "uma barra superior separada das abas". **O NARO não tinha abas.**
O README listava isso como não suportado, e não havia `struct Tab`, lista de abas nem
barra de abas no código — a janela tinha uma única página, uma barra de ferramentas
de 44 px e uma barra de status.

Portanto a tarefa não era só remover uma faixa: era criar as abas e, junto com elas,
substituir a barra de título do sistema por essa faixa. O trabalho foi feito nessa
ordem, e a premissa está registrada aqui em vez de silenciosamente reinterpretada.

## 1. Arquitetura encontrada

| Peça | Tecnologia | Onde |
|---|---|---|
| Janela e eventos | `winit` 0.30 | `crates/naro-app/src/main.rs` |
| Superfície | `softbuffer` (CPU) | idem |
| Desenho do chrome | pintado à mão em `Canvas` | `draw_chrome` |
| Layout do chrome | `chrome_layout(w, h)` | idem |
| Título | `Window::set_title` com decorações nativas do SO | `resumed` / `sync_title` |

A janela era criada com `Window::default_attributes()` — moldura do sistema, barra de
título nativa, botões nativos. Não havia nenhum recurso de integração em uso.

## 2. Abordagem escolhida

`with_decorations(false)` + reimplementar o que a moldura nativa dava, usando as APIs
que o próprio `winit` oferece:

| Comportamento nativo perdido | Reimplementação |
|---|---|
| Arrastar pela barra de título | `window.drag_window()` na área vazia da faixa de abas |
| Redimensionar pelas bordas | `window.drag_resize_window(direction)` a partir de uma faixa de 5 px nas bordas/cantos |
| Minimizar / maximizar / fechar | `set_minimized`, `set_maximized(!is_maximized())`, `event_loop.exit()` |
| Duplo clique maximiza | mesmo limiar de 400 ms da barra nativa |
| Cursores de redimensionamento | `CursorIcon::{Ew,Ns,Nwse,Nesw}Resize` conforme a borda |

Não foi usada nenhuma dependência nova. Não foi criada uma barra de título
"personalizada completa" com widgets: a faixa de abas **é** a barra de título, e os
três botões do sistema são desenhados no mesmo `Canvas` que já pintava o resto do
chrome, com ações reais na janela real.

## 3. Abas

- Modelo: a aba ativa mora em `App { engine, chrome, active_id }`; as inativas em
  `Vec<BackgroundTab>` com o próprio `Engine` e `Chrome`. Trocar de aba faz
  `mem::swap` — todo o caminho de eventos e desenho continua operando em campos
  simples, sem índice.
- `order: Vec<u64>` mantém a ordem visual independente da ordem de armazenamento.
- Abrir (`+` ou Ctrl+T), selecionar (clique), fechar (`×` ou Ctrl+W). Fechar a última
  aba fecha a janela, como em um navegador.
- Cada aba tem seu próprio `Engine`, logo seu próprio DOM, JS, histórico e scroll.
- Fetch em segundo plano carrega a aba de destino: `UserEvent::Fetched` passou a
  levar `tab: u64`, então uma aba em background não escreve na visível.
- A largura das abas encolhe com a quantidade (mín. 56 px, máx. 190 px) e o espaço
  livre à direita do `+` é superfície de arrasto.

## 4. Sistema de ícones (todos os botões)

Antes havia três linguagens visuais diferentes na mesma barra: setas tipográficas
(`"←"`, `"→"`), um arco geométrico para recarregar e quadrados estampados para o
`×`. O pedido de "trocar todos os botões por modernos" foi tratado como a unificação
disso num único conjunto.

`enum Icon { Back, Forward, Reload, Close, Plus, Minimize, Maximize, Restore }` com
uma única função `draw_icon(canvas, icon, cx, cy, size, color)`, um peso de traço
(`ICON_STROKE = 1.4`) e um raio de canto para os fundos de hover. Todos os botões do
chrome usam esse caminho: navegação, recarregar, fechar aba, nova aba e os três
controles da janela.

Estados padronizados: hover (`C_HOVER`), pressionado (`C_HOVER_PRESSED`), foco por
teclado (anel `C_ACCENT`), desabilitado (`C_DISABLED`). Só o botão fechar da janela
usa cor (`C_WIN_CLOSE_HOVER`), como nos navegadores.

Acessibilidade: como o chrome é pintado em pixels e não tem árvore de acessibilidade,
o rótulo vem de um tooltip desenhado (`Voltar (Alt+←)`, `Recarregar (F5)`,
`Avançar (Alt+→)`) e de navegação por teclado — **F6** cicla o foco pelos três
botões, **Enter/Espaço** ativam, **Esc** sai. O clique não deixa anel de foco, como
num navegador.

## 5. O bug que fazia os ícones parecerem "iguais aos antigos"

A primeira versão desenhava as linhas estampando quadrados de 1,5 px via
`fill_rect`, que arredonda para pixels inteiros. A 1× isso produz um traço de 2 px em
degraus — literalmente o mesmo aspecto pixelado de antes, só com outra forma.

Correção na primitiva, não no ícone: `Canvas::stroke_path` (máscara de cobertura por
distância ao segmento, com blend único e junções por `max`) e `Canvas::fill_triangle`
(4×4 supersampling). É o que dá antialiasing de verdade; os ícones passaram a usar
`stroke_path` em vez de estampar quadrados.

## 6. Ajustes visuais medidos, não supostos

Medi as caixas dos ícones na captura em vez de julgar no olho:

| Ícone | Antes | Depois |
|---|---|---|
| chevrons | 6×12 | 8×14 (mesma altura óptica do arco) |
| reload | 15×13 | 12×11 (mesmo peso dos demais) |
| controles da janela | 10×10 | 10×10 com caixa de 12 px |

O campo de endereço também deixou de ser retangular: `fill_round_rect` com raio
`altura/2`, que era a inconsistência mais visível ao lado dos botões arredondados.

## 7. Área útil

| Antes | Depois |
|---|---|
| barra de título do SO (~30 px) + toolbar 44 px | faixa de abas 34 px (que é a barra de título) + toolbar 40 px |

O conteúdo ganhou a altura da moldura do sistema e 4 px da toolbar, e a
`CONTENT_HEIGHT` das páginas não mudou (o viewport é recalculado a partir de
`CHROME_H`).

## 8. Arquivos alterados

| Arquivo | Alteração |
|---|---|
| `crates/naro-paint/src/lib.rs` | `stroke_path` (antialiased polyline) e `fill_triangle` (4×4 supersampling) |
| `crates/naro-app/src/main.rs` | abas (modelo, ciclo de vida, `order`, fetch por aba); `with_decorations(false)`; arrasto, redimensionamento, minimizar/maximizar/fechar reais; `enum Icon` + `draw_icon` unificado; tooltips; F6/Enter/Esc; campo de endereço arredondado; 6 testes de layout do chrome |

Nenhum arquivo do motor, do CSS, do layout ou da rede foi alterado. Nenhum
relatório, captura ou benchmark anterior foi apagado.

## 9. Testes

```
cargo test --workspace     225 passed, 0 failed   (219 antes; +6)
naro --compat              27 pass, 0 falhas, 1 não suportados (total 28)
```

Os 6 testes novos cobrem geometria do chrome, que é onde estavam os defeitos
possíveis e onde não há captura de tela para provar:

- `tab_strip_occupies_the_top_and_content_starts_below_it` — a faixa começa em y=0,
  o conteúdo começa exatamente depois dela e da toolbar, e a soma fecha a altura.
- `window_controls_sit_at_the_right_of_the_tab_row` — alinhamento à direita, mesma
  altura da faixa, ordem min/max/close, sem sobreposição.
- `tabs_never_overlap_each_other_or_the_window_controls` — de 1 a 12 abas, bordas
  compartilhadas (foi o teste que pegou a sobreposição de sub-pixel: `x + w` era
  calculado à parte e gerava 145,42857 + 0,00001).
- `tab_width_shrinks_as_tabs_are_added` — máximo com poucas, mínimo com muitas.
- `toolbar_buttons_do_not_intersect_the_address_bar`.
- `resize_border_is_inside_the_window`.

Páginas de referência sem regressão: `example.com` 820,60 e `rust-lang.org`
3968,98 — idênticos aos valores anteriores à tarefa.

## 10. Limitações por sistema operacional

- **Windows**: verificado nesta máquina. `set_decorations(false)` remove a moldura;
  arrasto, resize e os três botões funcionam; a janela maximizada cobre a tela sem
  faixa morta. Um detalhe: ao restaurar, o `winit` devolve a janela ao tamanho
  anterior sem animação — o mesmo que outros apps sem moldura fazem.
- **macOS**: não verificado. O `winit` remove a barra de título, mas o padrão da
  plataforma é manter os "semáforos" (fechar/minimizar/zoom) no canto superior
  esquerdo. Como os controles foram desenhados no canto **direito**, a janela ficaria
  sem os botões nativos e com botões desenhados em posição não-nativa. Correção
  necessária antes de suportar macOS: usar
  `WindowAttributesExtMacOS::with_titlebar_transparent` + `with_fullsize_content_view`
  e manter os semáforos, em vez de `decorations(false)`.
- **Linux (X11/Wayland)**: não verificado. `drag_window` e `drag_resize_window` são
  suportados pelo `winit`, mas no Wayland o arrasto iniciado pelo cliente depende do
  compositor; em alguns ambientes o movimento pode ser ignorado. Os botões
  desenhados continuam funcionando, então não há perda funcional, só de arrasto.
- **Acessibilidade**: o chrome é pixels. Os tooltips e o F6 cobrem o uso por teclado,
  mas leitores de tela não veem os botões. Resolver isso exigiria widgets nativos ou
  `accesskit`, o que é uma mudança maior do que esta tarefa.
- **O `<title>` da página continua indo para `set_title`**, que agora só aparece na
  barra de tarefas, já que a janela não tem barra de título. Isso é o comportamento
  esperado em navegadores com abas.

## 11. O que não foi feito

- Nenhuma fase do roadmap foi iniciada.
- Sem reordenação de abas por arrasto, sem abas fixadas, sem menu de contexto.
- Sem ícones para recarregar "parado" vs. "carregando" (não há indicador de
  progresso no NARO).
- A verificação visual foi feita por captura de tela do processo; não houve teste
  automatizado de arrasto ou de redimensionamento, que dependem de entrada real do
  sistema.
