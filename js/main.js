// ============================
// ANIMATIONS AU SCROLL
// ============================
const observerOptions = {
  threshold: 0.1,
  rootMargin: '0px 0px -50px 0px'
};

const observer = new IntersectionObserver((entries) => {
  entries.forEach(entry => {
    if (entry.isIntersecting) {
      entry.target.classList.add('animated');
      
      // Ajouter des effets spéciaux selon le type d'élément
      if (entry.target.classList.contains('service-card')) {
        setTimeout(() => {
          entry.target.style.transform += ' rotateY(5deg)';
          setTimeout(() => {
            entry.target.style.transform = entry.target.style.transform.replace(' rotateY(5deg)', '');
          }, 300);
        }, 500);
      }
    }
  });
}, observerOptions);

// Observer tous les éléments avec la classe animate-on-scroll
document.addEventListener('DOMContentLoaded', () => {
  const animatedElements = document.querySelectorAll('.animate-on-scroll');
  animatedElements.forEach(el => observer.observe(el));
});

// ============================
// HEADER SCROLL EFFECT
// ============================
window.addEventListener('scroll', () => {
  const header = document.getElementById('header');
  const scrolled = window.pageYOffset;
  
  if (scrolled > 100) {
    header.classList.add('scrolled');
  } else {
    header.classList.remove('scrolled');
  }
  
  // Effet parallax sur le hero
  const hero = document.querySelector('.hero');
  if (hero && scrolled < window.innerHeight) {
    hero.style.transform = `translateY(${scrolled * 0.5}px)`;
  }
});

// ============================
// MENU BURGER
// ============================
const burger = document.getElementById('burger');
const navMenu = document.getElementById('nav-menu');

if (burger && navMenu) {
  burger.addEventListener('click', () => {
    burger.classList.toggle('active');
    navMenu.classList.toggle('active');
    
    // Animation des éléments du menu
    if (navMenu.classList.contains('active')) {
      const menuItems = navMenu.querySelectorAll('li');
      menuItems.forEach((item, index) => {
        item.style.animation = `fadeInLeft 0.5s ease forwards ${index * 0.1}s`;
      });
    }
  });

  // Fermer le menu si on clique sur un lien
  navMenu.addEventListener('click', (e) => {
    if (e.target.tagName === 'A') {
      burger.classList.remove('active');
      navMenu.classList.remove('active');
    }
  });
}

// ============================
// SMOOTH SCROLL
// ============================
document.querySelectorAll('a[href^="#"]').forEach(anchor => {
  anchor.addEventListener('click', function (e) {
    e.preventDefault();
    const target = document.querySelector(this.getAttribute('href'));
    if (target) {
      target.scrollIntoView({
        behavior: 'smooth',
        block: 'start'
      });
      
      // Effet de highlight sur la section ciblée
      target.style.transition = 'box-shadow 0.5s ease';
      target.style.boxShadow = '0 0 30px rgba(106, 27, 154, 0.3)';
      setTimeout(() => {
        target.style.boxShadow = '';
      }, 1000);
    }
  });
});

// ============================
// PARTICULES FLOTTANTES
// ============================
function createParticles() {
  const particlesContainer = document.getElementById('particles');
  if (!particlesContainer) return;
  
  function createParticle() {
    const particle = document.createElement('div');
    particle.className = 'particle';
    particle.style.left = Math.random() * 100 + '%';
    particle.style.animationDelay = Math.random() * 15 + 's';
    particle.style.animationDuration = (Math.random() * 10 + 10) + 's';
    
    // Variation de taille et couleur
    const size = Math.random() * 6 + 2;
    particle.style.width = size + 'px';
    particle.style.height = size + 'px';
    
    const colors = ['rgba(106, 27, 154, 0.3)', 'rgba(142, 36, 170, 0.3)', 'rgba(171, 71, 188, 0.3)'];
    particle.style.background = colors[Math.floor(Math.random() * colors.length)];
    
    particlesContainer.appendChild(particle);

    // Supprimer la particule après l'animation
    setTimeout(() => {
      if (particle.parentNode) {
        particle.parentNode.removeChild(particle);
      }
    }, 25000);
  }

  // Créer une particule toutes les 3 secondes
  setInterval(createParticle, 3000);
  
  // Créer quelques particules au début
  for (let i = 0; i < 5; i++) {
    setTimeout(createParticle, i * 1000);
  }
}

// ============================
// PARALLAX EFFECT AVANCÉ (SANS ROTATION DES IMAGES)
// ============================
window.addEventListener('scroll', () => {
  const scrolled = window.pageYOffset;
  const parallaxElements = document.querySelectorAll('.parallax');
  
  parallaxElements.forEach((element, index) => {
    const speed = 0.5 + (index * 0.1);
    element.style.transform = `translateY(${scrolled * speed}px)`;
  });
  
  // EFFET DE ROTATION SUPPRIMÉ - Les images restent fixes
});

// ============================
// FORMULAIRE DE CONTACT ANIMÉ
// ============================
document.addEventListener('DOMContentLoaded', () => {
  const contactForm = document.querySelector('.contact-form');
  if (contactForm) {
    contactForm.addEventListener('submit', function(e) {
      e.preventDefault();
      
      const button = this.querySelector('button');
      const originalText = button.textContent;
      const inputs = this.querySelectorAll('input, textarea');
      
      // Animation de chargement
      button.innerHTML = '<div class="loading-spinner"></div>';
      button.disabled = true;
      
      // Animation des champs
      inputs.forEach((input, index) => {
        setTimeout(() => {
          input.style.transform = 'scale(0.95)';
          input.style.opacity = '0.7';
        }, index * 100);
      });
      
      setTimeout(() => {
        button.textContent = '✓ Message envoyé !';
        button.style.background = 'linear-gradient(135deg, #4CAF50, #45a049)';
        button.style.animation = 'pulse 0.5s ease';
        
        // Réinitialiser les champs
        inputs.forEach((input, index) => {
          setTimeout(() => {
            input.style.transform = 'scale(1)';
            input.style.opacity = '1';
            input.value = '';
          }, index * 100);
        });
        
        setTimeout(() => {
          button.textContent = originalText;
          button.style.background = '';
          button.style.animation = '';
          button.disabled = false;
        }, 2000);
      }, 1500);
    });

    // Animation des champs au focus
    const formInputs = contactForm.querySelectorAll('input, textarea');
    formInputs.forEach(input => {
      input.addEventListener('focus', function() {
        this.style.transform = 'translateY(-5px) scale(1.02)';
        this.style.boxShadow = '0 10px 30px rgba(106, 27, 154, 0.2)';
      });
      
      input.addEventListener('blur', function() {
        this.style.transform = 'translateY(0) scale(1)';
        this.style.boxShadow = '0 5px 15px rgba(0,0,0,0.1)';
      });
    });
  }
});

// ============================
// EFFECTS AVANCÉS
// ============================

// Effet de typing sur le titre principal
function typeWriter(element, text, speed = 100) {
  let i = 0;
  element.innerHTML = '';
  
  function type() {
    if (i < text.length) {
      element.innerHTML += text.charAt(i);
      i++;
      setTimeout(type, speed);
    }
  }
  type();
}

// Curseur personnalisé avec traînée
let mouseTrail = [];
document.addEventListener('mousemove', (e) => {
  mouseTrail.push({ x: e.clientX, y: e.clientY, time: Date.now() });
  
  // Limiter la longueur de la traînée
  if (mouseTrail.length > 20) {
    mouseTrail.shift();
  }
  
  // Créer les éléments de traînée
  mouseTrail.forEach((point, index) => {
    if (index === mouseTrail.length - 1) return;
    
    const trail = document.createElement('div');
    const opacity = (index / mouseTrail.length) * 0.5;
    const size = (index / mouseTrail.length) * 10 + 2;
    
    trail.style.cssText = `
      position: fixed;
      left: ${point.x}px;
      top: ${point.y}px;
      width: ${size}px;
      height: ${size}px;
      background: rgba(106, 27, 154, ${opacity});
      border-radius: 50%;
      pointer-events: none;
      z-index: 9999;
      transform: translate(-50%, -50%);
      animation: fadeOut 1s ease-out forwards;
    `;
    
    document.body.appendChild(trail);
    
    setTimeout(() => {
      if (trail.parentNode) {
        trail.parentNode.removeChild(trail);
      }
    }, 1000);
  });
});

// Animation de fadeOut pour les éléments de traînée
const trailStyle = document.createElement('style');
trailStyle.textContent = `
  @keyframes fadeOut {
    0% { opacity: 1; transform: translate(-50%, -50%) scale(1); }
    100% { opacity: 0; transform: translate(-50%, -50%) scale(0); }
  }
`;
document.head.appendChild(trailStyle);

// ============================
// ANIMATIONS INTERACTIVES
// ============================
document.addEventListener('DOMContentLoaded', () => {
  // Démarrer les particules
  createParticles();
  
  // Ajouter des effets hover avancés aux service cards
  document.querySelectorAll('.service-card').forEach(card => {
    card.addEventListener('mouseenter', function() {
      this.style.transform = 'translateY(-15px) scale(1.03) rotateY(5deg)';
      this.style.transition = 'all 0.4s cubic-bezier(0.175, 0.885, 0.32, 1.275)';
      
      // Ajouter un effet de particules sur hover
      createCardParticles(this);
    });
    
    card.addEventListener('mouseleave', function() {
      this.style.transform = 'translateY(0) scale(1) rotateY(0deg)';
    });
    
    // Effet de click
    card.addEventListener('click', function() {
      this.style.animation = 'pulse 0.6s ease';
      setTimeout(() => {
        this.style.animation = '';
      }, 600);
    });
  });

  // EFFET AMÉLIORÉ POUR LES BOUTONS (y compris "Contactez-nous")
  document.querySelectorAll('.btn').forEach(btn => {
    btn.addEventListener('mouseenter', function() {
      // Effet de soulèvement avec ombre plus prononcée
      this.style.transform = 'translateY(-6px) scale(1.05)';
      this.style.boxShadow = '0 15px 35px rgba(106, 27, 154, 0.4)';
      this.style.transition = 'all 0.3s cubic-bezier(0.175, 0.885, 0.32, 1.275)';
      
      // Effet de lueur
      this.style.filter = 'brightness(1.1)';
    });
    
    btn.addEventListener('mouseleave', function() {
      this.style.transform = 'translateY(0) scale(1)';
      this.style.boxShadow = '0 4px 15px rgba(106, 27, 154, 0.3)';
      this.style.filter = 'brightness(1)';
    });
    
    // Effet ripple au click
    btn.addEventListener('click', function(e) {
      const ripple = document.createElement('span');
      const rect = this.getBoundingClientRect();
      const size = Math.max(rect.width, rect.height);
      const x = e.clientX - rect.left - size / 2;
      const y = e.clientY - rect.top - size / 2;
      
      ripple.style.cssText = `
        position: absolute;
        width: ${size}px;
        height: ${size}px;
        left: ${x}px;
        top: ${y}px;
        background: rgba(255, 255, 255, 0.5);
        border-radius: 50%;
        transform: scale(0);
        animation: rippleEffect 0.6s ease-out;
        pointer-events: none;
      `;
      
      this.appendChild(ripple);
      
      setTimeout(() => {
        if (ripple.parentNode) {
          ripple.parentNode.removeChild(ripple);
        }
      }, 600);
    });
  });

  // Animation de compteur pour les statistiques
  function animateCounter(element, target, duration = 2000) {
    let start = 0;
    const increment = target / (duration / 16);
    
    function updateCounter() {
      start += increment;
      if (start < target) {
        element.textContent = Math.floor(start);
        requestAnimationFrame(updateCounter);
      } else {
        element.textContent = target;
      }
    }
    updateCounter();
  }

  // Effet de vague sur le hero
  const hero = document.querySelector('.hero');
  if (hero) {
    let waveOffset = 0;
    
    function animateWave() {
      waveOffset += 0.01;
      const heroContent = hero.querySelector('.hero-content');
      if (heroContent) {
        heroContent.style.transform = `translateY(${Math.sin(waveOffset) * 5}px)`;
      }
      requestAnimationFrame(animateWave);
    }
    animateWave();
  }

  // Animation du logo au scroll - SUPPRIMÉE
  // Le logo reste maintenant fixe sans rotation

  // Effet de text reveal sur les titres (SAUF la section contact)
  const titles = document.querySelectorAll('.section-title');
  titles.forEach(title => {
    // Ne pas appliquer l'effet sur le titre de la section contact (texte blanc)
    if (title.closest('.contact')) return;
    
    title.addEventListener('mouseenter', function() {
      this.style.background = 'linear-gradient(45deg, #6A1B9A, #8E24AA, #AB47BC)';
      this.style.backgroundSize = '200% auto';
      this.style.animation = 'gradientShift 2s ease infinite';
      this.style.webkitBackgroundClip = 'text';
      this.style.webkitTextFillColor = 'transparent';
    });
    
    title.addEventListener('mouseleave', function() {
      this.style.background = '';
      this.style.animation = '';
      this.style.webkitBackgroundClip = '';
      this.style.webkitTextFillColor = '';
    });
  });
});

// ============================
// PARTICULES POUR LES CARDS
// ============================
function createCardParticles(card) {
  const rect = card.getBoundingClientRect();
  
  for (let i = 0; i < 6; i++) {
    const particle = document.createElement('div');
    particle.style.cssText = `
      position: absolute;
      width: 4px;
      height: 4px;
      background: #6A1B9A;
      border-radius: 50%;
      pointer-events: none;
      z-index: 1000;
      left: ${rect.left + Math.random() * rect.width}px;
      top: ${rect.top + Math.random() * rect.height}px;
      animation: sparkleFloat 2s ease-out forwards;
    `;
    
    document.body.appendChild(particle);
    
    setTimeout(() => {
      if (particle.parentNode) {
        particle.parentNode.removeChild(particle);
      }
    }, 2000);
  }
}

// ============================
// ANIMATIONS CONTINUES
// ============================
setInterval(() => {
  // Ajouter des micro-animations aléatoires
  const cards = document.querySelectorAll('.service-card');
  cards.forEach((card, index) => {
    if (!card.matches(':hover')) {
      setTimeout(() => {
        card.style.transform += ` rotate(${Math.random() * 2 - 1}deg)`;
        setTimeout(() => {
          card.style.transform = card.style.transform.replace(/rotate\([^)]+\)/g, '');
        }, 200);
      }, index * 100);
    }
  });
}, 15000);

// ============================
// EFFET RIPPLE GLOBAL
// ============================
const rippleStyle = document.createElement('style');
rippleStyle.textContent = `
  @keyframes rippleEffect {
    0% {
      transform: scale(0);
      opacity: 1;
    }
    100% {
      transform: scale(4);
      opacity: 0;
    }
  }
`;
document.head.appendChild(rippleStyle);

// ============================
// LOADING SCREEN ANIMATION
// ============================
function showLoadingAnimation() {
  const loader = document.createElement('div');
  loader.style.cssText = `
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: linear-gradient(135deg, #6A1B9A, #8E24AA);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 10000;
    transition: opacity 0.5s ease;
  `;
  
  loader.innerHTML = `
    <div style="text-align: center; color: white;">
      <div class="loading-spinner" style="margin: 0 auto 20px; border-color: rgba(255,255,255,0.3); border-top-color: white;"></div>
      <h2 style="font-family: 'Playfair Display', serif; margin: 0;">Afya Rahma</h2>
      <p style="margin: 10px 0 0; opacity: 0.8;">Chargement en cours<span class="loading-dots"></span></p>
    </div>
  `;
  
  document.body.appendChild(loader);
  
  // Simuler le chargement
  setTimeout(() => {
    loader.style.opacity = '0';
    setTimeout(() => {
      if (loader.parentNode) {
        loader.parentNode.removeChild(loader);
      }
    }, 500);
  }, 2000);
}

// ============================
// ANIMATIONS DE SCROLL INFINI
// ============================
function infiniteScroll() {
  const sections = document.querySelectorAll('section');
  
  window.addEventListener('scroll', () => {
    sections.forEach(section => {
      const rect = section.getBoundingClientRect();
      const isVisible = rect.top < window.innerHeight && rect.bottom > 0;
      
      if (isVisible) {
        // Ajouter des effets dynamiques pendant le scroll
        const scrollProgress = (window.innerHeight - rect.top) / window.innerHeight;
        section.style.filter = `brightness(${0.8 + scrollProgress * 0.2})`;
      }
    });
  });
}

// ============================
// EASTER EGG - KONAMI CODE
// ============================
let konamiCode = [];
const konamiSequence = [38, 38, 40, 40, 37, 39, 37, 39, 66, 65]; // ↑↑↓↓←→←→BA

document.addEventListener('keydown', (e) => {
  konamiCode.push(e.keyCode);
  
  if (konamiCode.length > konamiSequence.length) {
    konamiCode.shift();
  }
  
  if (konamiCode.join(',') === konamiSequence.join(',')) {
    // Animation spéciale Easter Egg
    document.body.style.animation = 'rainbow 2s ease infinite';
    
    // Créer des confettis
    for (let i = 0; i < 100; i++) {
      createConfetti();
    }
    
    setTimeout(() => {
      document.body.style.animation = '';
    }, 5000);
    
    konamiCode = [];
  }
});

// Créer des confettis
function createConfetti() {
  const confetti = document.createElement('div');
  const colors = ['#6A1B9A', '#8E24AA', '#AB47BC', '#9C27B0', '#E91E63', '#FF9800'];
  
  confetti.style.cssText = `
    position: fixed;
    width: 10px;
    height: 10px;
    background: ${colors[Math.floor(Math.random() * colors.length)]};
    left: ${Math.random() * 100}%;
    top: -10px;
    z-index: 10000;
    pointer-events: none;
    animation: confettiFall ${Math.random() * 3 + 2}s ease-out forwards;
  `;
  
  document.body.appendChild(confetti);
  
  setTimeout(() => {
    if (confetti.parentNode) {
      confetti.parentNode.removeChild(confetti);
    }
  }, 5000);
}

// Animation arc-en-ciel pour l'Easter Egg
const rainbowStyle = document.createElement('style');
rainbowStyle.textContent = `
  @keyframes rainbow {
    0% { filter: hue-rotate(0deg); }
    100% { filter: hue-rotate(360deg); }
  }
  
  @keyframes confettiFall {
    0% {
      transform: translateY(-10px) rotate(0deg);
      opacity: 1;
    }
    100% {
      transform: translateY(100vh) rotate(720deg);
      opacity: 0;
    }
  }
`;
document.head.appendChild(rainbowStyle);

// ============================
// INITIALISATION GÉNÉRALE
// ============================
document.addEventListener('DOMContentLoaded', () => {
  // Activer le scroll infini
  infiniteScroll();
  
  // Animation d'entrée du site
  setTimeout(() => {
    document.body.style.opacity = '1';
    document.body.style.transform = 'translateY(0)';
  }, 100);
  
  // Ajouter des animations aléatoires périodiques
  setInterval(() => {
    const randomElements = document.querySelectorAll('.service-card, .btn, img');
    const randomElement = randomElements[Math.floor(Math.random() * randomElements.length)];
    
    if (randomElement && !randomElement.matches(':hover')) {
      randomElement.style.animation = 'pulse 0.5s ease';
      setTimeout(() => {
        randomElement.style.animation = '';
      }, 500);
    }
  }, 10000);
  
  // Observer les images pour lazy loading avec animation
  const imageObserver = new IntersectionObserver((entries) => {
    entries.forEach(entry => {
      if (entry.isIntersecting) {
        const img = entry.target;
        img.style.animation = 'zoomIn 1s ease forwards';
        imageObserver.unobserve(img);
      }
    });
  });
  
  document.querySelectorAll('img').forEach(img => {
    imageObserver.observe(img);
  });
  
  // Animation du texte wave sur les titres
  document.querySelectorAll('h1, h2, h3').forEach(title => {
    title.addEventListener('mouseenter', function() {
      const text = this.textContent;
      this.innerHTML = text.split('').map((char, index) => 
        `<span style="animation-delay: ${index * 0.1}s">${char}</span>`
      ).join('');
      this.classList.add('wave-text');
    });
    
    title.addEventListener('mouseleave', function() {
      this.classList.remove('wave-text');
      this.innerHTML = this.textContent;
    });
  });
});

// ============================
// ANIMATIONS DE PERFORMANCE
// ============================
let ticking = false;

function updateAnimations() {
  // Mise à jour des animations coûteuses
  const scrolled = window.pageYOffset;
  
  // Animation du background du hero
  const hero = document.querySelector('.hero');
  if (hero) {
    hero.style.backgroundPosition = `center ${scrolled * 0.5}px`;
  }
  
  ticking = false;
}

window.addEventListener('scroll', () => {
  if (!ticking) {
    requestAnimationFrame(updateAnimations);
    ticking = true;
  }
});

// ============================
// GESTION DES ANIMATIONS MOBILES
// ============================
function isMobile() {
  return window.innerWidth <= 768;
}

// Réduire les animations sur mobile pour les performances
if (isMobile()) {
  const style = document.createElement('style');
  style.textContent = `
    .particle { display: none; }
    .floating-particles { display: none; }
    * { animation-duration: 0.5s !important; }
  `;
  document.head.appendChild(style);
}

// ============================
// PRÉCHARGEMENT DES ANIMATIONS
// ============================
function preloadAnimations() {
  const preloadDiv = document.createElement('div');
  preloadDiv.style.cssText = `
    position: absolute;
    left: -9999px;
    opacity: 0;
    animation: fadeIn 0.1s;
  `;
  document.body.appendChild(preloadDiv);
  
  setTimeout(() => {
    if (preloadDiv.parentNode) {
      preloadDiv.parentNode.removeChild(preloadDiv);
    }
  }, 100);
}

// Lancer le préchargement
preloadAnimations();

// ============================
// DEBUG MODE (pour développement)
// ============================
if (window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1') {
  console.log('🎨 Mode Debug Animations Activé');
  
  // Ajouter des indicateurs visuels pour le debug
  window.debugAnimations = () => {
    document.querySelectorAll('.animate-on-scroll').forEach(el => {
      el.style.border = '2px dashed red';
    });
  };
  
  // Raccourci clavier pour activer/désactiver les animations
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'a') {
      document.body.style.animationPlayState = 
        document.body.style.animationPlayState === 'paused' ? 'running' : 'paused';
    }
  });
}